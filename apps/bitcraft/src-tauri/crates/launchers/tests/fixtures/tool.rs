use std::io::{Read, Write};

fn main() {
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
    if std::env::var_os("TEST_ECHO_STDIO").is_some() {
        std::io::stdout().write_all(&input).unwrap();
        eprintln!("fixture stderr");
    } else {
        let arguments = std::env::args().skip(1).collect::<Vec<_>>();
        println!("{}", arguments.join("\n"));
        println!(
            "DOBJ_HOME={}",
            std::env::var("DOBJ_HOME").unwrap_or_default()
        );
        println!(
            "DOBJD_URL={}",
            std::env::var("DOBJD_URL").unwrap_or_default()
        );
    }
    std::process::exit(23);
}
