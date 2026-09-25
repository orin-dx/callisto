//! The one color/table decision, shared by every command's text-format output.
//!
//! `NO_COLOR` (non-empty) always wins over everything else; otherwise
//! `CLICOLOR_FORCE`/`FORCE_COLOR` force color on (anstream's own auto-detection
//! doesn't see `FORCE_COLOR`, hence checking it explicitly here); otherwise
//! anstream's own TTY auto-detection decides. Color and box-drawing table
//! formatting are never decided independently -- callers gate both off the
//! same [`enabled`] result.

/// Resolves the color/table `ColorChoice` from the real process environment and stdout.
pub fn resolve() -> anstream::ColorChoice {
    resolve_with(&|name| std::env::var(name).ok())
}

fn resolve_with(var: &dyn Fn(&str) -> Option<String>) -> anstream::ColorChoice {
    if var("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return anstream::ColorChoice::Never;
    }
    if var("CLICOLOR_FORCE").is_some() || var("FORCE_COLOR").is_some() {
        return anstream::ColorChoice::Always;
    }
    anstream::AutoStream::choice(&std::io::stdout())
}

/// True when color and box-drawing table formatting should render for stdout.
pub fn enabled() -> bool {
    resolve() != anstream::ColorChoice::Never
}

/// Wraps stdout in an `AutoStream` using the one resolved `ColorChoice`, so any
/// ANSI codes a renderer emits are stripped when color is disabled.
pub fn stdout() -> anstream::AutoStream<std::io::Stdout> {
    anstream::AutoStream::new(std::io::stdout(), resolve())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anstream::ColorChoice;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// AC-05: `NO_COLOR` set to a non-empty value always disables color, regardless of the force vars.
    #[test]
    fn no_color_non_empty_wins_over_force_vars() {
        let env = vars(&[("NO_COLOR", "1"), ("CLICOLOR_FORCE", "1"), ("FORCE_COLOR", "1")]);
        assert_eq!(resolve_with(&|k| env.get(k).cloned()), ColorChoice::Never);
    }

    /// AC-04: `NO_COLOR` set but empty does not disable color; `CLICOLOR_FORCE` forces it on.
    #[test]
    fn no_color_empty_does_not_suppress_clicolor_force() {
        let env = vars(&[("NO_COLOR", ""), ("CLICOLOR_FORCE", "1")]);
        assert_eq!(resolve_with(&|k| env.get(k).cloned()), ColorChoice::Always);
    }

    /// AC-04: `CLICOLOR_FORCE` alone forces color on.
    #[test]
    fn clicolor_force_forces_color_on() {
        let env = vars(&[("CLICOLOR_FORCE", "1")]);
        assert_eq!(resolve_with(&|k| env.get(k).cloned()), ColorChoice::Always);
    }

    /// AC-04: `FORCE_COLOR` alone forces color on -- anstream's own auto-detection
    /// doesn't see this variable, which is exactly the blind spot this module exists to close.
    #[test]
    fn force_color_forces_color_on() {
        let env = vars(&[("FORCE_COLOR", "1")]);
        assert_eq!(resolve_with(&|k| env.get(k).cloned()), ColorChoice::Always);
    }

    /// AC-05: neither `NO_COLOR` nor a force var set falls back to anstream's own
    /// auto-detection, which always resolves to a concrete Always/Never, never Auto.
    #[test]
    fn no_env_signal_falls_back_to_concrete_auto_detection() {
        let env: HashMap<String, String> = HashMap::new();
        let choice = resolve_with(&|k| env.get(k).cloned());
        assert_ne!(choice, ColorChoice::Auto);
    }

    /// `enabled()` mirrors `resolve()`: only `Never` disables it.
    #[test]
    fn enabled_matches_resolve_not_never() {
        assert_eq!(enabled(), resolve() != ColorChoice::Never);
    }
}
