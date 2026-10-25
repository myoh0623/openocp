use std::sync::atomic::{AtomicBool, Ordering};

// One desktop app language, independent of connection profiles/browser locales.
static KOREAN: AtomicBool = AtomicBool::new(false);

pub(crate) fn is_korean() -> bool {
    KOREAN.load(Ordering::Relaxed)
}

fn parse(language: &str) -> Result<bool, String> {
    match language {
        "en" => Ok(false),
        "ko" => Ok(true),
        _ => Err("Unsupported app language. Choose en or ko.".into()),
    }
}

pub(crate) fn set(language: &str) -> Result<(), String> {
    KOREAN.store(parse(language)?, Ordering::Relaxed);
    Ok(())
}

macro_rules! tr {
    ($en:literal, $ko:literal) => {
        if $crate::language::is_korean() {
            $ko
        } else {
            $en
        }
    };
}

macro_rules! tr_format {
    ($en:literal, $ko:literal $(, $args:expr)* $(,)?) => {
        if $crate::language::is_korean() {
            format!($ko $(, $args)*)
        } else {
            format!($en $(, $args)*)
        }
    };
}

macro_rules! tr_bail {
    ($en:literal, $ko:literal $(, $args:expr)* $(,)?) => {
        if $crate::language::is_korean() {
            anyhow::bail!($ko $(, $args)*)
        } else {
            anyhow::bail!($en $(, $args)*)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_supported_app_languages() {
        assert_eq!(parse("en"), Ok(false));
        assert_eq!(parse("ko"), Ok(true));
        assert!(parse("en-US").is_err());
        assert!(parse("fr").is_err());
    }
}
