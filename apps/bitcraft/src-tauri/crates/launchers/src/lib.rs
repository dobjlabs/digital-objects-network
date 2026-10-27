use std::{
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const DEFAULT_HTTP_PORT: u16 = 7727;

pub fn resolve_home(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if explicit
        .as_ref()
        .is_some_and(|path| path.as_os_str().is_empty())
    {
        bail!("--home requires a non-empty path");
    }
    let path = match explicit.or_else(|| {
        std::env::var_os("BITCRAFT_HOME")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
    }) {
        Some(path) => path,
        _ => dirs::home_dir()
            .context("Cannot resolve the home directory")?
            .join(".bitcraft"),
    };
    Ok(if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    })
}

pub fn connection_path(home: &Path) -> PathBuf {
    home.join("connection.json")
}

fn tool_home(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if explicit.is_some() || std::env::var_os("BITCRAFT_HOME").is_some_and(|path| !path.is_empty())
    {
        return resolve_home(explicit);
    }
    let directory = std::env::current_dir()?;
    for ancestor in directory.ancestors() {
        if connection_path(ancestor).try_exists()? {
            return Ok(ancestor.to_path_buf());
        }
    }
    resolve_home(None)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub format_version: u32,
    pub http_url: String,
    pub mcp_url: String,
    pub state_root: PathBuf,
    pub instance_id: String,
}

impl Connection {
    pub fn new(state_root: PathBuf, instance_id: String, port: u16) -> Result<Self> {
        let mcp_port = port
            .checked_add(1)
            .context("Cannot derive the adjacent MCP port")?;
        let connection = Self {
            format_version: 1,
            http_url: format!("http://127.0.0.1:{port}"),
            mcp_url: format!("http://127.0.0.1:{mcp_port}/mcp"),
            state_root,
            instance_id,
        };
        connection.validate()?;
        Ok(connection)
    }

    fn validate(&self) -> Result<()> {
        if self.format_version != 1 || self.instance_id.is_empty() || !self.state_root.is_absolute()
        {
            bail!("Invalid Bitcraft connection file");
        }
        for (address, path) in [(&self.http_url, "/"), (&self.mcp_url, "/mcp")] {
            let url = reqwest::Url::parse(address).context("Invalid Bitcraft endpoint")?;
            if url.scheme() != "http"
                || !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
                || url.port_or_known_default().is_none_or(|port| port == 0)
                || !url.username().is_empty()
                || url.password().is_some()
                || url.path() != path
                || url.query().is_some()
                || url.fragment().is_some()
            {
                bail!("Bitcraft connection endpoints must be loopback HTTP URLs");
            }
        }
        let http = reqwest::Url::parse(&self.http_url)?;
        let mcp = reqwest::Url::parse(&self.mcp_url)?;
        if http.host_str() != mcp.host_str()
            || http
                .port_or_known_default()
                .and_then(|port| port.checked_add(1))
                != mcp.port_or_known_default()
        {
            bail!("Bitcraft's MCP endpoint must use the port adjacent to its HTTP endpoint");
        }
        Ok(())
    }

    pub fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("No active Bitcraft connection at {}. Start Bitcraft and run from its home directory, or select it with --home PATH.", path.display()))?;
        let connection: Self =
            serde_json::from_slice(&bytes).context("Invalid Bitcraft connection file")?;
        connection.validate()?;
        Ok(connection)
    }

    pub fn publish(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let parent = path
            .parent()
            .context("Connection file has no parent directory")?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer(&mut temporary, self)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary.persist(path)?;
        Ok(())
    }

    pub fn remove(&self, path: &Path) -> Result<()> {
        match fs::read(path) {
            Ok(bytes) => {
                let current: Self = serde_json::from_slice(&bytes)?;
                if current.instance_id == self.instance_id {
                    fs::remove_file(path)?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    pub async fn verify(&self) -> Result<()> {
        self.validate()?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()?;
        let response = client
            .get(format!("{}/healthz", self.http_url.trim_end_matches('/')))
            .send()
            .await
            .context("Bitcraft is not reachable. Start the Bitcraft app first")?
            .error_for_status()
            .context("Bitcraft's daemon is not ready")?;
        let health: serde_json::Value = response
            .json()
            .await
            .context("Invalid daemon health response")?;
        if health["ok"].as_bool() != Some(true)
            || health["instance_id"].as_str() != Some(&self.instance_id)
        {
            bail!(
                "The responding daemon does not match Bitcraft's active instance. Refusing to connect"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub enum Tool {
    Cli,
    McpProxy,
}

impl Tool {
    fn name(self) -> &'static str {
        match self {
            Self::Cli => "bitcraft-dobj",
            Self::McpProxy => "bitcraft-dobj-mcp-proxy",
        }
    }
}

fn arguments(mut args: Vec<OsString>) -> Result<(Option<PathBuf>, Vec<OsString>)> {
    let mut home = None;
    if args.first().is_some_and(|arg| arg == "--home") {
        if args.len() < 2 || args[1].is_empty() {
            bail!("--home requires a path");
        }
        home = Some(PathBuf::from(args.remove(1)));
        args.remove(0);
    } else if let Some(first) = args.first().and_then(|arg| arg.to_str())
        && let Some(path) = first.strip_prefix("--home=")
    {
        if path.is_empty() {
            bail!("--home requires a path");
        }
        home = Some(PathBuf::from(path));
        args.remove(0);
    }
    if args.first().is_some_and(|arg| arg == "--") {
        args.remove(0);
    }
    Ok((home, args))
}

fn validate_arguments(tool: Tool, args: &[OsString]) -> Result<()> {
    let before_separator = args.iter().take_while(|arg| *arg != "--");
    for arg in before_separator {
        if let Some(arg) = arg.to_str()
            && (matches!(arg, "--url" | "--port" | "--home")
                || arg.starts_with("--url=")
                || arg.starts_with("--port=")
                || arg.starts_with("--home="))
        {
            bail!(
                "{} selects Bitcraft's endpoint automatically. Put --home PATH before the tool arguments",
                tool.name()
            );
        }
    }
    if matches!(tool, Tool::Cli) {
        let mut args = args.iter().take_while(|arg| *arg != "--");
        let command = args.find(|arg| !arg.to_string_lossy().starts_with('-'));
        if command.is_some_and(|arg| {
            matches!(
                arg.to_str(),
                Some("start" | "stop" | "status" | "logs" | "ui" | "update")
            )
        }) {
            bail!(
                "Manage the Bitcraft daemon and app updates through the Bitcraft desktop application"
            );
        }
    }
    Ok(())
}

fn executable(tool: Tool) -> Result<PathBuf> {
    let name = format!("{}-inner{}", tool.name(), std::env::consts::EXE_SUFFIX);
    let executable = std::env::current_exe()?;
    let sibling = executable
        .parent()
        .context("Cannot locate the launcher directory")?
        .join(&name);
    if sibling.is_file() {
        return Ok(sibling);
    }
    #[cfg(debug_assertions)]
    {
        let staged = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../binaries")
            .join(format!(
                "{}-inner-{}{}",
                tool.name(),
                env!("BITCRAFT_TARGET"),
                std::env::consts::EXE_SUFFIX
            ));
        if staged.is_file() {
            return Ok(staged);
        }
    }
    bail!(
        "Bundled tool missing at {}. Reinstall or rebuild the complete Bitcraft package",
        sibling.display()
    )
}

pub fn launch(tool: Tool) -> Result<i32> {
    let (home, args) = arguments(std::env::args_os().skip(1).collect())?;
    let information_only = args
        .iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| matches!(arg.to_str(), Some("--help" | "-h" | "--version" | "-V")))
        || args == [OsString::from("help")];
    validate_arguments(tool, &args)?;
    let mut command = Command::new(executable(tool)?);
    if information_only {
        if args
            .iter()
            .any(|arg| matches!(arg.to_str(), Some("--help" | "-h")))
        {
            eprintln!(
                "{} [--home PATH] [tool arguments]\nUses --home, BITCRAFT_HOME, a connection.json in the current directory or its ancestors, then ~/.bitcraft. Verifies the running app's identity; endpoint overrides and standalone lifecycle commands are unavailable.",
                tool.name()
            );
        }
    } else {
        let connection = Connection::read(&connection_path(&tool_home(home)?))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(connection.verify())?;
        drop(runtime);
        command.arg("--url").arg(match tool {
            Tool::Cli => &connection.http_url,
            Tool::McpProxy => &connection.mcp_url,
        });
        command.env("DOBJ_HOME", &connection.state_root);
    }
    command
        .env_remove("DOBJD_URL")
        .env_remove("DOBJD_BIN")
        .env_remove("DOBJD_PORT");
    command.args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec()).with_context(|| format!("Could not run {}", tool.name()))
    }
    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .with_context(|| format!("Could not run {}", tool.name()))?;
        Ok(status.code().unwrap_or(1))
    }
}

pub fn main_for(tool: Tool) {
    let code = match launch(tool) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{}: {error:#}", tool.name());
            1
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_publication_is_complete_and_cleanup_respects_ownership() {
        let home = tempfile::tempdir().unwrap();
        let path = connection_path(home.path());
        let original =
            Connection::new(home.path().to_path_buf(), "first".into(), DEFAULT_HTTP_PORT).unwrap();
        original.publish(&path).unwrap();
        assert_eq!(Connection::read(&path).unwrap().instance_id, "first");
        let replacement = Connection::new(
            home.path().to_path_buf(),
            "second".into(),
            DEFAULT_HTTP_PORT,
        )
        .unwrap();
        replacement.publish(&path).unwrap();
        original.remove(&path).unwrap();
        assert_eq!(Connection::read(&path).unwrap().instance_id, "second");
        replacement.remove(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn invalid_connections_are_not_published() {
        let home = tempfile::tempdir().unwrap();
        let mut connection =
            Connection::new(home.path().to_path_buf(), "id".into(), DEFAULT_HTTP_PORT).unwrap();
        connection.http_url = "https://example.com".into();
        assert!(connection.publish(&connection_path(home.path())).is_err());
        assert!(!connection_path(home.path()).exists());
    }

    #[test]
    fn connection_requires_matching_adjacent_endpoints_and_accepts_port_eighty() {
        let home = tempfile::tempdir().unwrap();
        let mut connection = Connection::new(home.path().to_path_buf(), "id".into(), 80).unwrap();
        connection.mcp_url = "http://127.0.0.1:7718/mcp".into();
        assert!(connection.validate().is_err());
        assert!(resolve_home(Some(PathBuf::new())).is_err());
    }

    #[test]
    fn endpoint_and_standalone_lifecycle_overrides_are_rejected() {
        for arg in [
            "--url",
            "--url=http://127.0.0.1:7717",
            "--port=7718",
            "start",
            "stop",
            "update",
            "logs",
            "status",
            "ui",
        ] {
            assert!(
                validate_arguments(Tool::Cli, &[arg.into()]).is_err(),
                "{arg}"
            );
        }
        assert!(
            validate_arguments(
                Tool::Cli,
                &["settings".into(), "set".into(), "--mcp".into(), "on".into()]
            )
            .is_ok()
        );
    }
}
