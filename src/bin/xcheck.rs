//! Cross-check the palette resolver against `omarchy-theme-color --all`.
//!
//! Development tool: run `cargo run --bin xcheck` on a machine with Omarchy
//! installed to confirm f3note resolves the same palette every other themed
//! application on the system resolves. Not part of the shipped editor.
use f3note::theme;
use std::process::Command;

fn main() {
    let mut themes: Vec<_> = std::fs::read_dir("/usr/share/omarchy/themes")
        .expect("omarchy themes directory")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    themes.sort();

    let (mut agree, mut differ) = (0u32, 0u32);
    for t in themes {
        let colors = t.join("colors.toml");
        if !colors.exists() {
            continue;
        }
        let Some(p) = theme::Palette::load(&colors) else {
            continue;
        };
        let out = Command::new("omarchy-theme-color")
            .args(["--file", colors.to_str().unwrap(), "--all"])
            .output()
            .expect("omarchy-theme-color");
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let Some((key, want)) = line.split_once('\t') else {
                continue;
            };
            if key == "theme_type" || key == "mode" {
                continue;
            }
            match p.get(key) {
                Some(got) if got == want => agree += 1,
                got => {
                    differ += 1;
                    if differ <= 12 {
                        println!(
                            "{:<16} {:<22} nosso={:?} omarchy={:?}",
                            t.file_name().unwrap().to_string_lossy(),
                            key,
                            got,
                            want
                        );
                    }
                }
            }
        }
    }
    println!("\nchaves iguais: {agree}   diferentes: {differ}");
}
