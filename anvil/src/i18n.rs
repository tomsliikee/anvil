//! Translations through gettext. Strings are written in English in the
//! code and looked up in `po/<lang>.po`, compiled into the Flatpak. The
//! language follows the system unless Settings pins English or German.

pub const DOMAIN: &str = "anvil";

/// Binds the domain; with a pinned language sets `LANGUAGE` first, which
/// gettext reads on every lookup.
pub fn init(pinned: Option<&str>) {
    if let Some(lang) = pinned {
        // SAFETY: called on the main thread before GTK or any other thread
        // starts, so nothing reads the environment concurrently.
        unsafe { std::env::set_var("LANGUAGE", lang) };
    }
    let dir = std::env::var("ANVIL_LOCALEDIR").unwrap_or_else(|_| "/app/share/locale".into());
    let _ = gettextrs::bindtextdomain(DOMAIN, dir);
    let _ = gettextrs::bind_textdomain_codeset(DOMAIN, "UTF-8");
    let _ = gettextrs::textdomain(DOMAIN);
}

pub fn tr(text: &str) -> String {
    gettextrs::gettext(text)
}

pub fn ntr(one: &str, many: &str, n: u32) -> String {
    gettextrs::ngettext(one, many, n)
}

/// `tr` with `{}` placeholders filled in order.
pub fn trf(text: &str, args: &[&str]) -> String {
    let mut s = tr(text);
    for a in args {
        if let Some(i) = s.find("{}") {
            s.replace_range(i..i + 2, a);
        }
    }
    s
}
