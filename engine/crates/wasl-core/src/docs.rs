//! Export targets for Google-native documents (SRS FR-C9).

/// The format a Google document is exported to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ExportTarget {
    pub mime: &'static str,
    pub extension: &'static str,
}

pub const GOOGLE_DOC: &str = "application/vnd.google-apps.document";
pub const GOOGLE_SHEET: &str = "application/vnd.google-apps.spreadsheet";
pub const GOOGLE_SLIDES: &str = "application/vnd.google-apps.presentation";
pub const GOOGLE_DRAWING: &str = "application/vnd.google-apps.drawing";
pub const GOOGLE_SCRIPT: &str = "application/vnd.google-apps.script";
pub const GOOGLE_JAM: &str = "application/vnd.google-apps.jam";

const DOCX: ExportTarget = ExportTarget {
    mime: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    extension: "docx",
};
const XLSX: ExportTarget = ExportTarget {
    mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    extension: "xlsx",
};
const PPTX: ExportTarget = ExportTarget {
    mime: "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    extension: "pptx",
};
const PNG: ExportTarget = ExportTarget {
    mime: "image/png",
    extension: "png",
};
const PDF: ExportTarget = ExportTarget {
    mime: "application/pdf",
    extension: "pdf",
};
const SCRIPT_JSON: ExportTarget = ExportTarget {
    mime: "application/vnd.google-apps.script+json",
    extension: "json",
};

/// The export target for a Google MIME type, or `None` when it cannot be exported (forms, sites…).
pub fn export_target(google_mime: &str, prefer_pdf: bool) -> Option<ExportTarget> {
    Some(match (google_mime, prefer_pdf) {
        (GOOGLE_DOC, false) => DOCX,
        (GOOGLE_SHEET, false) => XLSX,
        (GOOGLE_SLIDES, false) => PPTX,
        (GOOGLE_DRAWING, false) => PNG,
        (GOOGLE_DOC | GOOGLE_SHEET | GOOGLE_SLIDES | GOOGLE_DRAWING, true) => PDF,
        (GOOGLE_SCRIPT, _) => SCRIPT_JSON,
        (GOOGLE_JAM, _) => PDF,
        _ => return None,
    })
}

/// Whether some Google file on Drive could be exported to the local file `name` in this format:
/// its extension is one an export writes.
pub fn is_export_name(name: &str, prefer_pdf: bool) -> bool {
    export_stem(name, prefer_pdf).is_some()
}

/// The name of the Google file on Drive that would be exported to the local file `name` in this
/// format: `Notes` for `Notes.docx`. `None` when no export writes that name.
pub fn export_stem(name: &str, prefer_pdf: bool) -> Option<&str> {
    let googles = [
        GOOGLE_DOC,
        GOOGLE_SHEET,
        GOOGLE_SLIDES,
        GOOGLE_DRAWING,
        GOOGLE_SCRIPT,
        GOOGLE_JAM,
    ];
    googles
        .into_iter()
        .filter_map(|mime| export_target(mime, prefer_pdf))
        .find_map(|t| {
            let stem = name.get(..name.len().checked_sub(t.extension.len() + 1)?)?;
            (!stem.is_empty() && export_local_name(stem, t).eq_ignore_ascii_case(name)).then_some(stem)
        })
}

/// Local file name for an exported doc: `Budget 2026.xlsx`. An existing extension is not duplicated.
pub fn export_local_name(name: &str, target: ExportTarget) -> String {
    if name.to_lowercase().ends_with(&format!(".{}", target.extension)) {
        name.to_owned()
    } else {
        format!("{name}.{}", target.extension)
    }
}

/// What a Google file is called in a distinct export name: `Google Doc`.
pub fn google_kind(google_mime: &str) -> &'static str {
    match google_mime {
        GOOGLE_DOC => "Google Doc",
        GOOGLE_SHEET => "Google Sheet",
        GOOGLE_SLIDES => "Google Slides",
        GOOGLE_DRAWING => "Google Drawing",
        GOOGLE_SCRIPT => "Apps Script",
        GOOGLE_JAM => "Jamboard",
        _ => "Google file",
    }
}

/// The plain export name without its extension: `Report` for `Report.docx`.
fn export_name_stem(name: &str, target: ExportTarget) -> String {
    let plain = export_local_name(name, target);
    plain
        .len()
        .checked_sub(target.extension.len() + 1)
        .and_then(|end| plain.get(..end))
        .unwrap_or(&plain)
        .to_owned()
}

/// Local file name for an export whose plain name ([`export_local_name`]) is already another
/// file's: `Report (Google Doc).docx` for `n` 1, then `Report (Google Doc 2).docx`, so a Google
/// file and a real file never share a name.
pub fn export_distinct_name(name: &str, google_mime: &str, target: ExportTarget, n: u32) -> String {
    let stem = export_name_stem(name, target);
    let kind = google_kind(google_mime);
    match n {
        0 | 1 => format!("{stem} ({kind}).{}", target.extension),
        n => format!("{stem} ({kind} {n}).{}", target.extension),
    }
}

/// Whether `file_name` is one of the [`export_distinct_name`]s of this Google file.
pub fn is_export_distinct_name(file_name: &str, name: &str, google_mime: &str, target: ExportTarget) -> bool {
    let head = format!("{} ({}", export_name_stem(name, target), google_kind(google_mime));
    let tail = format!(").{}", target.extension);
    file_name
        .strip_prefix(&head)
        .and_then(|rest| rest.strip_suffix(&tail))
        .is_some_and(|n| {
            n.is_empty()
                || n.strip_prefix(' ').is_some_and(|d| {
                    !d.is_empty() && !d.starts_with('0') && d.bytes().all(|b| b.is_ascii_digit())
                })
        })
}
