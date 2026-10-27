use anyhow::{Context, Result};

#[cfg(target_os = "linux")]
pub fn launch(url: &str) -> Result<bool> {
    use gio::prelude::AppInfoExt;

    let (scheme, _) = url.split_once(':').context("Invalid agent link")?;
    let Some(application) = gio::AppInfo::default_for_uri_scheme(scheme) else {
        return Ok(false);
    };
    application.launch_uris(&[url], None::<&gio::AppLaunchContext>)?;
    Ok(true)
}

#[cfg(target_os = "macos")]
pub fn launch(url: &str) -> Result<bool> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSURL};

    let url = NSURL::URLWithString(&NSString::from_str(url)).context("Invalid agent link")?;
    let workspace = NSWorkspace::sharedWorkspace();
    if workspace.URLForApplicationToOpenURL(&url).is_none() {
        return Ok(false);
    }
    anyhow::ensure!(
        workspace.openURL(&url),
        "macOS could not open the agent app"
    );
    Ok(true)
}

#[cfg(target_os = "windows")]
pub fn launch(url: &str) -> Result<bool> {
    use windows::{
        Foundation::Uri,
        System::{LaunchQuerySupportStatus, LaunchQuerySupportType, Launcher},
        core::HSTRING,
    };

    let uri = Uri::CreateUri(&HSTRING::from(url)).context("Invalid agent link")?;
    if Launcher::QueryUriSupportAsync(&uri, LaunchQuerySupportType::Uri)?.join()?
        != LaunchQuerySupportStatus::Available
    {
        return Ok(false);
    }
    anyhow::ensure!(
        Launcher::LaunchUriAsync(&uri)?.join()?,
        "Windows could not open the agent app"
    );
    Ok(true)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn launch(_url: &str) -> Result<bool> {
    Ok(false)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn an_unregistered_scheme_returns_unavailable_without_launching() {
        assert!(!super::launch("bitcraft-test-no-handler-a1c60491://new").unwrap());
    }
}
