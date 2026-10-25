//! The third-party licence notices Help > About shows, written by `scripts/gen-notices`.

pub const TEXT: &str = include_str!("../assets/THIRD-PARTY-NOTICES.txt");

#[cfg(test)]
mod tests {
    use super::TEXT;

    #[test]
    fn every_locked_crate_has_a_notice() {
        #[derive(serde::Deserialize)]
        struct Lock {
            package: Vec<Package>,
        }
        #[derive(serde::Deserialize)]
        struct Package {
            name: String,
            version: String,
            source: Option<String>,
        }
        let lock: Lock = toml::from_str(include_str!("../../../Cargo.lock")).unwrap();
        let missing: Vec<_> = lock
            .package
            .iter()
            .filter(|p| p.source.is_some())
            .map(|p| format!("\n{} {}: ", p.name, p.version))
            .filter(|line| !TEXT.contains(line))
            .collect();
        assert!(
            missing.is_empty(),
            "run scripts/gen-notices; missing:{}",
            missing.concat()
        );
    }

    #[test]
    fn carries_the_texts_that_need_shipping() {
        for text in [
            // whisper.cpp
            "MIT License\n\nCopyright (c) 2023-2024 The ggml authors",
            // webpki-roots
            "Community Data License Agreement - Permissive - Version 2.0",
            // FFmpeg
            "GNU LESSER GENERAL PUBLIC LICENSE",
            // Inter, Noto Sans CJK
            "SIL OPEN FONT LICENSE Version 1.1",
        ] {
            assert!(TEXT.contains(text), "missing {text:?}");
        }
        for (file, _) in crate::samples::CLIPS {
            assert!(
                TEXT.contains(&format!("\n{file}: ")),
                "missing sample {file}"
            );
        }
    }
}
