use protocol_model::{
    lifecycle::{LifecycleModel, Scenario},
    reservations::ReservationModel,
};
use stateright::Model;

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let (name, address) = match args.as_slice() {
        [command, name] if command == "explore" => (name.as_str(), "127.0.0.1:3001"),
        [command, name, address] if command == "explore" => (name.as_str(), address.as_str()),
        _ => usage(),
    };
    match name {
        "leases" => {
            ReservationModel::leases().checker().serve(address);
        }
        "recovery" => {
            ReservationModel::recovery().checker().serve(address);
        }
        _ => {
            let scenario = match name {
                "effects" => Scenario::Effects,
                "reorg" => Scenario::Reorg,
                "expiry" => Scenario::Expiry,
                _ => usage(),
            };
            LifecycleModel::new(scenario).checker().serve(address);
        }
    }
}

fn usage() -> ! {
    eprintln!("Usage: protocol-model explore <leases|recovery|effects|reorg|expiry> [address]");
    std::process::exit(2)
}
