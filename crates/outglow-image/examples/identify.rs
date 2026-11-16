fn main() {
    for p in std::env::args().skip(1) {
        let img = std::fs::read(&p).expect("read");
        match outglow_image::board::identify(&img) {
            Ok(id) => println!("{p}: {id:?}"),
            Err(e) => println!("{p}: ERR {e}"),
        }
    }
}
