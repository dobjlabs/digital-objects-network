use std::io::{self, Read};

pub fn install_if_requested() -> io::Result<()> {
    if !std::env::args_os().any(|argument| argument == "--exit-on-stdin-close") {
        return Ok(());
    }

    std::thread::Builder::new()
        .name("parent-lifetime".into())
        .spawn(|| {
            let stdin = io::stdin();
            let mut input = stdin.lock();
            let mut buffer = [0; 256];
            loop {
                match input.read(&mut buffer) {
                    Ok(0) => std::process::exit(0),
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => {
                        eprintln!("Parent lifetime pipe failed: {error}");
                        std::process::exit(1);
                    }
                }
            }
        })?;
    Ok(())
}

#[cfg(test)]
#[path = "parent_lifetime_tests.rs"]
mod tests;
