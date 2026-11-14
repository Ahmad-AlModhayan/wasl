//! Offline vault recovery (`wasld vault decrypt`) invoked directly or via `wasl vault decrypt`.
//!
//! Uses only `wasl_crypto::recover`, the passphrase, and standard I/O (SRS FR-V3); no running
//! daemon state or network is touched.

use std::fmt::Display;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use wasl_crypto::CryptoError;
use wasl_crypto::recover::{Event, Reason, Vault, decrypt_tree};
use zeroize::Zeroizing;

const OK: i32 = 0;
const GENERAL: i32 = 1;
const USAGE: i32 = 2;
const NOT_FOUND: i32 = 4;
const AUTH: i32 = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    En,
    Ar,
}

fn lang_from_env() -> Lang {
    let parse = |v: &str| {
        let v = v.trim().to_ascii_lowercase();
        if v.starts_with("ar") {
            Some(Lang::Ar)
        } else if v.starts_with("en") {
            Some(Lang::En)
        } else {
            None
        }
    };
    if let Ok(v) = std::env::var("WASL_LANG")
        && let Some(l) = parse(&v)
    {
        return l;
    }
    for k in ["LC_ALL", "LANG"] {
        if let Ok(v) = std::env::var(k) {
            let t = v.trim();
            if !t.is_empty() {
                return if t.to_ascii_lowercase().starts_with("ar") {
                    Lang::Ar
                } else {
                    Lang::En
                };
            }
        }
    }
    Lang::En
}

fn clean(text: &str) -> String {
    text.chars()
        .map(|c| {
            let invisible = matches!(
                c,
                '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
            );
            if c.is_control() || invisible { '?' } else { c }
        })
        .collect()
}

fn fill(template: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && after.as_bytes().get(digits) == Some(&b'}') {
            let i: usize = after[..digits].parse().unwrap_or(usize::MAX);
            match args.get(i) {
                Some(a) => out.push_str(&clean(&a.to_string())),
                None => out.push_str(&rest[open..open + digits + 2]),
            }
            rest = &after[digits + 1..];
        } else {
            out.push('{');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn msg(lang: Lang, en: &str, ar: &str, args: &[&dyn Display]) -> String {
    fill(if lang == Lang::Ar { ar } else { en }, args)
}

fn err_line(lang: Lang, detail: &str) -> String {
    msg(lang, "Error: {0}", "خطأ: {0}", &[&detail])
}

fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64 / 1024.0;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    format!("{v:.1} {}", UNITS[unit])
}

fn reason(lang: Lang, r: &Reason) -> String {
    match r {
        Reason::NotVaultItem => msg(lang, "not an item of this vault", "ليس من عناصر هذه الخزنة", &[]),
        Reason::AlreadyExists => msg(
            lang,
            "already in the output folder (use --overwrite)",
            "موجود بالفعل في مجلد الإخراج (استخدم --overwrite)",
            &[],
        ),
        Reason::SameName => msg(
            lang,
            "another item in this folder has the same name",
            "عنصر آخر في هذا المجلد يحمل الاسم نفسه",
            &[],
        ),
        Reason::UnsafeName => msg(
            lang,
            "its name cannot be used on this computer",
            "لا يمكن استخدام اسمه على هذا الحاسوب",
            &[],
        ),
        Reason::NoDirId => msg(
            lang,
            "the folder's .wasl-dir is missing or does not verify, so its contents cannot be named",
            "ملف .wasl-dir الخاص بالمجلد مفقود أو لا يمكن التحقق منه، فلا يمكن معرفة أسماء محتوياته",
            &[],
        ),
        Reason::MissingChunk(what) => msg(
            lang,
            "a piece of the file is missing: {0}",
            "جزء من الملف مفقود: {0}",
            &[what],
        ),
        Reason::Damaged(what) => msg(lang, "modified or damaged: {0}", "معدَّل أو تالف: {0}", &[what]),
        Reason::Io(what) => msg(
            lang,
            "could not read or write: {0}",
            "تعذّرت القراءة أو الكتابة: {0}",
            &[what],
        ),
    }
}

fn read_passphrase(lang: Lang, from_stdin: bool) -> Result<Zeroizing<String>, (i32, String)> {
    let prompt = msg(lang, "Vault passphrase: ", "عبارة مرور الخزنة: ", &[]);
    if from_stdin {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| (GENERAL, e.to_string()))?;
        while line.ends_with(['\n', '\r']) {
            line.pop();
        }
        return Ok(Zeroizing::new(line));
    }
    if !std::io::stdin().is_terminal() {
        return Err((
            USAGE,
            msg(
                lang,
                "No terminal to ask for the passphrase on: pipe it in with --passphrase-stdin.",
                "لا توجد طرفية لطلب عبارة المرور: مرّرها عبر --passphrase-stdin.",
                &[],
            ),
        ));
    }
    let _ = std::io::stderr().flush();
    rpassword::prompt_password(&prompt)
        .map(Zeroizing::new)
        .map_err(|e| (GENERAL, e.to_string()))
}

struct DecryptArgs {
    folder: PathBuf,
    output: PathBuf,
    passphrase_stdin: bool,
    overwrite: bool,
}

fn parse_args(args: &[String]) -> Result<Option<DecryptArgs>, String> {
    let mut folder: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut passphrase_stdin = false;
    let mut overwrite = false;
    for a in args {
        match a.as_str() {
            "-h" | "--help" => return Ok(None),
            "--passphrase-stdin" => passphrase_stdin = true,
            "--overwrite" => overwrite = true,
            flag if flag.starts_with('-') => return Err(format!("unknown flag {flag}")),
            pos => {
                if folder.is_none() {
                    folder = Some(PathBuf::from(pos));
                } else if output.is_none() {
                    output = Some(PathBuf::from(pos));
                } else {
                    return Err(format!("unexpected argument {pos}"));
                }
            }
        }
    }
    let Some(folder) = folder else {
        return Err("missing <folder> and <output>".into());
    };
    let Some(output) = output else {
        return Err("missing <output>".into());
    };
    Ok(Some(DecryptArgs {
        folder,
        output,
        passphrase_stdin,
        overwrite,
    }))
}

/// Runs `wasld vault decrypt` and returns the CLI process exit code.
pub fn run_decrypt(args: &[String]) -> i32 {
    let lang = lang_from_env();
    let parsed = match parse_args(args) {
        Ok(Some(a)) => a,
        Ok(None) => {
            println!("usage: wasld vault decrypt <folder> <output> [--passphrase-stdin] [--overwrite]");
            return OK;
        }
        Err(e) => {
            eprintln!("{}", err_line(lang, &e));
            return USAGE;
        }
    };
    match decrypt(
        lang,
        &parsed.folder,
        &parsed.output,
        parsed.passphrase_stdin,
        parsed.overwrite,
    ) {
        Ok(code) => code,
        Err((code, detail)) => {
            eprintln!("{}", err_line(lang, &detail));
            code
        }
    }
}

fn decrypt(
    lang: Lang,
    folder: &Path,
    output: &Path,
    passphrase_stdin: bool,
    overwrite: bool,
) -> Result<i32, (i32, String)> {
    if !folder.is_dir() {
        return Err((
            NOT_FOUND,
            msg(
                lang,
                "No such folder: {0}",
                "لا يوجد مجلد بهذا المسار: {0}",
                &[&folder.display()],
            ),
        ));
    }
    let vault = Vault::load(folder).map_err(|e| match e {
        CryptoError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => (
            NOT_FOUND,
            msg(
                lang,
                "{0} has no vault.json. Download the whole vault folder from Drive.",
                "لا يحتوي {0} على vault.json. نزّل مجلد الخزنة كاملًا من Drive.",
                &[&folder.display()],
            ),
        ),
        e => (
            GENERAL,
            msg(
                lang,
                "This vault cannot be read: {0}",
                "تعذّرت قراءة هذه الخزنة: {0}",
                &[&e],
            ),
        ),
    })?;
    let name = vault.name().to_owned();
    let fmt_label = vault.format_label();
    let created = vault.created().to_owned();
    println!(
        "{}",
        msg(
            lang,
            "Vault: {0} (format v{1}, created {2})",
            "الخزنة: {0} (الصيغة v{1}، أُنشئت {2})",
            &[&name, &fmt_label, &created],
        )
    );
    let pass = read_passphrase(lang, passphrase_stdin)?;
    if pass.is_empty() {
        return Err((
            USAGE,
            msg(lang, "The passphrase is empty.", "عبارة المرور فارغة.", &[]),
        ));
    }
    let keys = vault
        .unlock(&pass)
        .map_err(|e| {
            (
                GENERAL,
                msg(
                    lang,
                    "This vault cannot be read: {0}",
                    "تعذّرت قراءة هذه الخزنة: {0}",
                    &[&e],
                ),
            )
        })?
        .ok_or_else(|| {
            (
                AUTH,
                msg(
                    lang,
                    "Wrong passphrase for this vault.",
                    "عبارة المرور غير صحيحة لهذه الخزنة.",
                    &[],
                ),
            )
        })?;
    println!("{}", msg(lang, "Passphrase: ok", "عبارة المرور: صحيحة", &[]));
    println!(
        "{}",
        msg(
            lang,
            "Decrypting into {0}",
            "جارٍ فك التشفير إلى {0}",
            &[&output.display()],
        )
    );
    let summary = decrypt_tree(folder, output, &keys, overwrite, |ev| match ev {
        Event::File(path, _) => {
            println!("{}", msg(lang, "  ok      {0}", "  تم      {0}", &[&path]))
        }
        Event::Failed(path, why) => {
            let r = reason(lang, why);
            eprintln!(
                "{}",
                msg(lang, "  FAILED  {0}: {1}", "  فشل     {0}: {1}", &[&path, &r])
            )
        }
        Event::Folder(_) | Event::Skipped(_, _) => {}
    })
    .map_err(|e| (GENERAL, e.to_string()))?;
    let bytes = size_text(summary.bytes);
    println!(
        "{}",
        msg(
            lang,
            "{0} folders, {1} files, {2} decrypted.",
            "المجلدات: {0}، الملفات: {1}، فُكّ تشفير {2}.",
            &[&summary.folders, &summary.files, &bytes],
        )
    );
    if !summary.skipped.is_empty() {
        let len = summary.skipped.len();
        println!("{}", msg(lang, "Skipped {0} items:", "تُخطّي {0} عنصر:", &[&len]));
        for (path, why) in &summary.skipped {
            let r = reason(lang, why);
            println!("{}", msg(lang, "  - {0}: {1}", "  - {0}: {1}", &[path, &r]));
        }
    }
    if summary.failed.is_empty() {
        return Ok(OK);
    }
    let len = summary.failed.len();
    println!(
        "{}",
        msg(
            lang,
            "{0} items could not be decrypted:",
            "تعذّر فك تشفير {0} عنصر:",
            &[&len],
        )
    );
    for (path, why) in &summary.failed {
        let r = reason(lang, why);
        println!("{}", msg(lang, "  - {0}: {1}", "  - {0}: {1}", &[path, &r]));
    }
    Ok(GENERAL)
}
