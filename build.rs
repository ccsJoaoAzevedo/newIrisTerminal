fn main() {
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        // Tem que ser o arquivo .ico aqui, o Windows não aceita .png no executável
        res.set_icon("assets/icon.ico");
        res.compile().unwrap();

        // No toolchain GNU (x86_64-pc-windows-gnu), o winres compila o ícone
        // para OUT_DIR/resource.o e empacota numa lib estática linkada via
        // `-lresource`. Só que esse objeto só tem dados de recurso (sem
        // símbolo que alguém referencie), e o `ld` só puxa membros de uma lib
        // estática que resolvem símbolo pendente — então ele descarta o
        // objeto ao resolver a lib e o ícone some do .exe sem erro de build.
        // Linkar o .o diretamente (fora de arquivo .a) evita essa seleção por
        // símbolo: um objeto passado direto pro linker é sempre incluído.
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
            let out_dir = std::env::var("OUT_DIR").unwrap();
            println!("cargo:rustc-link-arg-bins={out_dir}/resource.o");
        }
    }

    // Isso avisa o Cargo para recompilar o exe se você trocar a imagem depois
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
}
