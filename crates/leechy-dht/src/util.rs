use rand::Rng;

pub fn generate_string(length: usize) -> String {
    let mut rng = rand::thread_rng();
    let mut s = String::with_capacity(length);
    s.extend((0..length).map(|_| rng.sample(rand::distributions::Alphanumeric) as char));
    s
}
