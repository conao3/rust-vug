bitflags::bitflags! {
    struct Status: u8 {
        const OK = 1;
    }
}

fn main() {
    println!("{}", Status::OK.bits());
}
