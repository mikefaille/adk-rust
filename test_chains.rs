fn main() {
    let x = Some(10);
    if let Some(v) = x && v > 5 {
        println!("Works!");
    }
}
