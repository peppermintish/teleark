//! Build-time bridge from TeleArk's Fluent catalogs to the English setup wizard.
use std::{error::Error, fs, io};
use teleark_i18n::{Localizer, MessageArgs, MessageId, SupportedLocale};

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args_os().nth(1).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "expected an output file path")
    })?;
    let localizer = Localizer::new(SupportedLocale::FALLBACK)?;
    let args = MessageArgs::new().with("code", "%1").with("log", "%2");
    let mut output = String::from("; Generated with teleark-i18n.\n[CustomMessages]\n");
    for (name, id) in [
        ("InstallerPreparing", "installer-preparing"),
        ("InstallerCancelled", "installer-cancelled"),
        ("InstallerFailed", "installer-failed"),
    ] {
        let message = localizer.translate_with(MessageId::new(id), &args)?;
        output.push_str(&format!("english.{name}={}\n", message.replace('\n', "%n")));
    }
    fs::write(path, output)?;
    Ok(())
}
