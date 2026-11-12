mod hub;
mod util;

fn main() {
    let total = hub::hub() + util::helper();
    println!("{total}");
}
