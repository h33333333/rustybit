use rand::{distributions::Alphanumeric, Rng};

pub fn generate_peer_id() -> String {
    let mut rng = rand::thread_rng();
    let mut peer_id = String::with_capacity(20);
    peer_id.push_str("RustyBit-");
    peer_id.extend((0..11).map(|_| rng.sample(Alphanumeric) as char));
    peer_id
}
