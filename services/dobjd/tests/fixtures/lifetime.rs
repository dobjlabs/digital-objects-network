use std::{
    fs,
    net::TcpListener,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

fn main() {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let directory = Path::new(&arguments[1]);
    if arguments[0] == "host" {
        let _daemon = Command::new(std::env::current_exe().unwrap())
            .args([
                "daemon",
                &arguments[1],
                &arguments[2],
                &arguments[3],
                "--exit-on-stdin-close",
            ])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        loop {
            std::thread::park();
        }
    }

    parent_lifetime::install_if_requested().unwrap();
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(10));
        std::process::exit(99);
    });
    let port = arguments[3].parse::<u16>().unwrap();
    let _listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).unwrap();
    fs::write(
        directory.join("port"),
        _listener.local_addr().unwrap().port().to_string(),
    )
    .unwrap();
    fs::write(directory.join("stage"), &arguments[2]).unwrap();
    if arguments[2] == "startup" {
        std::thread::sleep(Duration::from_secs(30));
    }
    loop {
        std::thread::park();
    }
}
