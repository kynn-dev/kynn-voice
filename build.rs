fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("img/kynn-voice.ico");
        res.compile().expect("Failed to compile Windows resources");
    }
}
