use std::process;

fn main() {
    if let Err(err) = happ::run() {
        eprintln!("happ failed: {err}");
        process::exit(1);
    }
}
