use rand::distributions::Alphanumeric;
use rand::Rng;

pub fn generate_node_id() -> [u8; 20] {
    let mut id = [0u8; 20];
    id[0..9].copy_from_slice(b"RustyBit-");
    let mut rng = rand::thread_rng();
    (0..11).for_each(|idx| {
        let idx = idx + 9;
        id[idx] = rng.sample(Alphanumeric);
    });
    id
}
