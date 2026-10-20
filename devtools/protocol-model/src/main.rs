use protocol_model::handshake::{self, Fault, Scenario};
use stateright::Model;

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let (name, address) = match args.as_slice() {
        [command, name] if command == "explore" => (name.as_str(), "127.0.0.1:3001"),
        [command, name, address] if command == "explore" => (name.as_str(), address.as_str()),
        _ => usage(),
    };
    let scenario = match name {
        "contention" => Scenario::Contention,
        "recovery" => Scenario::Recovery,
        "store-restart" => Scenario::StoreRestart,
        _ => usage(),
    };
    handshake::model(scenario, Fault::None)
        .checker()
        .serve(address);
}

fn usage() -> ! {
    eprintln!("Usage: protocol-model explore <contention|recovery|store-restart> [address]");
    std::process::exit(2)
}
