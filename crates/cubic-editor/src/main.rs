fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Err(error) = cubic_editor::run() {
            eprintln!("editor exited with an error: {error}");
            std::process::exit(1);
        }
    }
}
