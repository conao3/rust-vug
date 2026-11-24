fn main() {
    assert_eq!(std::env::var("CARGO_NET_OFFLINE").as_deref(), Ok("true"));
    let routes = std::fs::read_to_string("/proc/net/route").unwrap();
    assert_eq!(routes.lines().count(), 1);
}
