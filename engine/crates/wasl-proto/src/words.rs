//! The catalogue of status and error words (ADR-0003, `docs/STATUS_WORDS.md`).
//!
//! Every sentence Wasl shows about a status or an error lives here, in English and Arabic. An entry
//! has a stable key (the part scripts and agents read), the words with plural forms, the typed values
//! its words take, one suggested fix as a typed action, and a translator's note. [`render`] turns an
//! entry and its values into words in the caller's language; apps and the CLI keep no wording of their own.

use serde::{Deserialize, Serialize};

/// The languages the catalogue speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Lang {
    En,
    Ar,
}

impl Lang {
    /// `ar`, `ar_SA.UTF-8`, `ar-EG` give Arabic and `en…` gives English. Anything else gives `None`;
    /// callers fall back to English.
    pub fn parse(s: &str) -> Option<Lang> {
        let s = s.trim().to_ascii_lowercase();
        if s.starts_with("ar") {
            Some(Lang::Ar)
        } else if s.starts_with("en") {
            Some(Lang::En)
        } else {
            None
        }
    }
}

/// The one thing an entry suggests doing. Safe, reversible fixes run on one tap; the rest only open
/// the place to act. Never renames, deletes or moves a file for the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Fix {
    Retry,
    SignInAgain,
    OpenFolder,
    OpenStorageSettings,
    OpenDriveStorage,
    OpenToRename,
    UnlockVault,
    GrantAccessAgain,
    UpdateWasl,
    /// Opens the screen that shows what waits for the person's decision: the first preview, a
    /// stopped plan, files changed in two places.
    Review,
}

/// A value an entry's words take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Folder,
    File,
    Device,
    Count,
}

impl Slot {
    fn name(self) -> &'static str {
        match self {
            Slot::Folder => "folder",
            Slot::File => "file",
            Slot::Device => "device",
            Slot::Count => "count",
        }
    }
}

/// The typed values a rendered entry was filled with, kept beside the words so an agent never
/// parses a sentence. Absent values read as a plain stand-in in the words ("this folder").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Values {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
}

impl Values {
    pub fn folder(mut self, v: impl Into<String>) -> Values {
        self.folder = Some(v.into());
        self
    }
    pub fn file(mut self, v: impl Into<String>) -> Values {
        self.file = Some(v.into());
        self
    }
    pub fn device(mut self, v: impl Into<String>) -> Values {
        self.device = Some(v.into());
        self
    }
    pub fn count(mut self, v: u64) -> Values {
        self.count = Some(v);
        self
    }
}

/// One language's words for an entry: the plural forms it uses. `other` is always there.
#[derive(Debug, Clone, Copy)]
pub struct Forms {
    pub zero: Option<&'static str>,
    pub one: Option<&'static str>,
    pub two: Option<&'static str>,
    pub few: Option<&'static str>,
    pub many: Option<&'static str>,
    pub other: &'static str,
}

impl Forms {
    /// A sentence with no plural forms.
    pub const fn single(other: &'static str) -> Forms {
        Forms {
            zero: None,
            one: None,
            two: None,
            few: None,
            many: None,
            other,
        }
    }
    /// English: one and other.
    pub const fn one_other(one: &'static str, other: &'static str) -> Forms {
        Forms {
            zero: None,
            one: Some(one),
            two: None,
            few: None,
            many: None,
            other,
        }
    }
    /// Arabic: zero, one, two, few, many and other.
    pub const fn six(
        zero: &'static str,
        one: &'static str,
        two: &'static str,
        few: &'static str,
        many: &'static str,
        other: &'static str,
    ) -> Forms {
        Forms {
            zero: Some(zero),
            one: Some(one),
            two: Some(two),
            few: Some(few),
            many: Some(many),
            other,
        }
    }

    fn pick(&self, cat: Plural) -> &'static str {
        let chosen = match cat {
            Plural::Zero => self.zero,
            Plural::One => self.one,
            Plural::Two => self.two,
            Plural::Few => self.few,
            Plural::Many => self.many,
            Plural::Other => None,
        };
        chosen.unwrap_or(self.other)
    }

    #[cfg(test)]
    fn all(&self) -> impl Iterator<Item = &'static str> {
        [
            self.zero,
            self.one,
            self.two,
            self.few,
            self.many,
            Some(self.other),
        ]
        .into_iter()
        .flatten()
    }
}

/// CLDR plural categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plural {
    Zero,
    One,
    Two,
    Few,
    Many,
    Other,
}

/// The plural category of `n` in `lang`.
pub fn plural(lang: Lang, n: u64) -> Plural {
    match lang {
        Lang::En => {
            if n == 1 {
                Plural::One
            } else {
                Plural::Other
            }
        }
        Lang::Ar => match n {
            0 => Plural::Zero,
            1 => Plural::One,
            2 => Plural::Two,
            _ => match n % 100 {
                3..=10 => Plural::Few,
                11..=99 => Plural::Many,
                _ => Plural::Other,
            },
        },
    }
}

/// One catalogue entry.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    /// Stable and documented; never reworded once shipped.
    pub key: &'static str,
    pub en: Forms,
    pub ar: Forms,
    /// For translators: where this shows and what the values are.
    pub note: &'static str,
    pub fix: Option<Fix>,
    /// The values the words take.
    pub slots: &'static [Slot],
}

/// The key of the entry an unknown error gets.
pub const FALLBACK: &str = "error.unknown";
/// The key of the entry a request failure the catalogue can't name gets.
pub const REQUEST_FALLBACK: &str = "request.other";

const fn e(
    key: &'static str,
    en: Forms,
    ar: Forms,
    fix: Option<Fix>,
    slots: &'static [Slot],
    note: &'static str,
) -> Entry {
    Entry {
        key,
        en,
        ar,
        note,
        fix,
        slots,
    }
}

use Fix::*;
use Slot::{Count, Device, File, Folder};

/// Every entry. Keep the keys unique; `docs/STATUS_WORDS.md` is generated from this table.
pub const ENTRIES: &[Entry] = &[
    // ---- the whole-app status
    e(
        "overall.synced",
        Forms::single("Synced"),
        Forms::single("تمت المزامنة"),
        None,
        &[],
        "Short status of the whole app when every folder is up to date.",
    ),
    e(
        "overall.syncing",
        Forms::single("Syncing"),
        Forms::single("تجري المزامنة"),
        None,
        &[],
        "Short status of the whole app while any folder is syncing.",
    ),
    e(
        "overall.paused",
        Forms::single("Paused"),
        Forms::single("متوقف مؤقتًا"),
        None,
        &[],
        "Short status of the whole app when syncing is paused.",
    ),
    e(
        "overall.waiting",
        Forms::single("Waiting"),
        Forms::single("في الانتظار"),
        None,
        &[],
        "Short status of the whole app when folders wait (no network, a schedule, a first preview).",
    ),
    e(
        "overall.needs_you",
        Forms::single("Needs you"),
        Forms::single("يحتاج إليك"),
        None,
        &[],
        "Short status of the whole app when something needs the person.",
    ),
    e(
        "overall.idle",
        Forms::single("No folders yet"),
        Forms::single("لا توجد مجلدات بعد"),
        None,
        &[],
        "Short status of the whole app before any folder is added.",
    ),
    e(
        "status.folders_need_you",
        Forms::one_other("{count} folder needs you", "{count} folders need you"),
        Forms::six(
            "لا مجلدات تحتاج إليك",
            "مجلد واحد يحتاج إليك",
            "مجلدان يحتاجان إليك",
            "{count} مجلدات تحتاج إليك",
            "{count} مجلدًا يحتاج إليك",
            "{count} مجلد يحتاج إليك",
        ),
        None,
        &[Count],
        "Badge or heading on the home screen. {count} is the number of folders that need the person.",
    ),
    e(
        "overall.partly_synced",
        Forms::single("Partly synced"),
        Forms::single("تمت المزامنة جزئيًا"),
        None,
        &[],
        "Short status of the whole app when a folder's last run moved some files and not others. Not 'synced', not 'failed'.",
    ),
    e(
        "status.items_need_you",
        Forms::one_other("{count} thing needs you", "{count} things need you"),
        Forms::six(
            "لا شيء يحتاج إليك",
            "شيء واحد يحتاج إليك",
            "شيئان يحتاجان إليك",
            "{count} أشياء تحتاج إليك",
            "{count} شيئًا يحتاج إليك",
            "{count} شيء يحتاج إليك",
        ),
        None,
        &[Count],
        "Badge or heading on the home screen. {count} is the number of needs-you items across all folders (one folder can hold several).",
    ),
    // ---- one folder's status (docs/STATUS_WORDS.md, issue #29)
    e(
        "folder.synced",
        Forms::single("{folder} is up to date."),
        Forms::single("{folder} محدَّث."),
        None,
        &[Folder],
        "A folder whose last run moved everything and which has nothing waiting.",
    ),
    e(
        "folder.syncing",
        Forms::single("Syncing {folder}…"),
        Forms::single("تجري مزامنة {folder}…"),
        None,
        &[Folder],
        "A folder is syncing now.",
    ),
    e(
        "folder.off",
        Forms::single("{folder} is turned off."),
        Forms::single("{folder} غير مفعّل."),
        None,
        &[Folder],
        "The person turned the folder off; nothing syncs.",
    ),
    e(
        "folder.paused",
        Forms::single("{folder} is paused."),
        Forms::single("{folder} متوقف مؤقتًا."),
        None,
        &[Folder],
        "The person paused the folder; it syncs again when resumed.",
    ),
    e(
        "folder.waiting_no_network",
        Forms::single("{folder} will sync when Wasl is back online."),
        Forms::single("ستتم مزامنة {folder} عندما يعود وصل إلى الاتصال بالإنترنت."),
        Some(Retry),
        &[Folder],
        "The device is offline, or the last run couldn't reach Google Drive. Nothing is lost; the next sync carries on.",
    ),
    e(
        "folder.waiting_schedule",
        Forms::single("{folder} is waiting for the right time to sync."),
        Forms::single("{folder} ينتظر الوقت المناسب للمزامنة."),
        None,
        &[Folder],
        "The folder's own conditions hold the next run (Wi-Fi, charging, battery, VPN), or its first sync hasn't come round yet.",
    ),
    e(
        "folder.waiting_device",
        Forms::single("{folder} will sync when {device} is back online."),
        Forms::single("ستتم مزامنة {folder} عندما يعود {device} إلى الاتصال."),
        None,
        &[Folder, Device],
        "A folder that syncs only between devices waits for a device that is offline now. {device} is that device's name.",
    ),
    e(
        "folder.cant_reach_device",
        Forms::single("Wasl can't reach {device}, so {folder} isn't synced there."),
        Forms::single("لا يستطيع وصل الوصول إلى {device}، لذلك لم تتم مزامنة {folder} هناك."),
        None,
        &[Folder, Device],
        "A folder that syncs only between devices has a device Wasl hasn't reached for a long time (or never). {device} is that device's name.",
    ),
    // ---- a device in "my devices"
    e(
        "device.online",
        Forms::single("Online"),
        Forms::single("متصل"),
        None,
        &[Device],
        "Status of a device in the list of the person's devices: connected now and not syncing.",
    ),
    e(
        "device.syncing",
        Forms::single("Syncing"),
        Forms::single("تجري المزامنة"),
        None,
        &[Device],
        "Status of a device in the list of the person's devices: connected now and a folder is syncing with it.",
    ),
    e(
        "device.last_seen_now",
        Forms::single("Last seen a moment ago"),
        Forms::single("آخر ظهور قبل لحظات"),
        None,
        &[Device],
        "Status of a device that is offline and was connected less than a minute ago.",
    ),
    e(
        "device.last_seen_minutes",
        Forms::one_other("Last seen {count} minute ago", "Last seen {count} minutes ago"),
        Forms::six(
            "آخر ظهور قبل أقل من دقيقة",
            "آخر ظهور قبل دقيقة واحدة",
            "آخر ظهور قبل دقيقتين",
            "آخر ظهور قبل {count} دقائق",
            "آخر ظهور قبل {count} دقيقة",
            "آخر ظهور قبل {count} دقيقة",
        ),
        None,
        &[Device, Count],
        "Status of a device that is offline: when it was last connected, in minutes (1 to 59). {count} is the number of minutes.",
    ),
    e(
        "device.last_seen_hours",
        Forms::one_other("Last seen {count} hour ago", "Last seen {count} hours ago"),
        Forms::six(
            "آخر ظهور قبل أقل من ساعة",
            "آخر ظهور قبل ساعة واحدة",
            "آخر ظهور قبل ساعتين",
            "آخر ظهور قبل {count} ساعات",
            "آخر ظهور قبل {count} ساعة",
            "آخر ظهور قبل {count} ساعة",
        ),
        None,
        &[Device, Count],
        "Status of a device that is offline: when it was last connected, in hours (1 to 23). {count} is the number of hours.",
    ),
    e(
        "device.last_seen_days",
        Forms::one_other("Last seen {count} day ago", "Last seen {count} days ago"),
        Forms::six(
            "آخر ظهور قبل أقل من يوم",
            "آخر ظهور قبل يوم واحد",
            "آخر ظهور قبل يومين",
            "آخر ظهور قبل {count} أيام",
            "آخر ظهور قبل {count} يومًا",
            "آخر ظهور قبل {count} يوم",
        ),
        None,
        &[Device, Count],
        "Status of a device that is offline: when it was last connected, in days (1 to 30). {count} is the number of days.",
    ),
    e(
        "device.cant_reach",
        Forms::single("Can't reach"),
        Forms::single("تعذّر الوصول إليه"),
        None,
        &[Device],
        "Status of a device that was never reached from here, or not for more than 30 days.",
    ),
    e(
        "device.this_needs_update",
        Forms::single("This device needs an update to sync with {device}. Update Wasl on this device."),
        Forms::single("يحتاج هذا الجهاز إلى تحديث للمزامنة مع {device}. حدّث وصل على هذا الجهاز."),
        Some(UpdateWasl),
        &[Device],
        "A connection to another device was refused because this device's Wasl is too old for it. {device} is the other device's name, or its ID when this device has no name for it.",
    ),
    e(
        "device.peer_needs_update",
        Forms::single("{device} needs an update to sync with this device. Update Wasl on {device}."),
        Forms::single("يحتاج {device} إلى تحديث للمزامنة مع هذا الجهاز. حدّث وصل على {device}."),
        None,
        &[Device],
        "A connection to another device was refused because that device's Wasl is too old for this one. {device} is its name, or its ID when this device has no name for it. No fix here: the update happens on the other device.",
    ),
    e(
        "folder.needs_you",
        Forms::single("{folder} needs you."),
        Forms::single("{folder} يحتاج إليك."),
        None,
        &[Folder],
        "Heading of a folder that has needs-you items; each item has its own entry.",
    ),
    e(
        "folder.first_preview",
        Forms::single("Look over what Wasl will do in {folder}, then start syncing."),
        Forms::single("راجع ما سيفعله وصل في {folder} ثم ابدأ المزامنة."),
        Some(Review),
        &[Folder],
        "A new folder waits for the person to start it from the first preview. Nothing moves until then.",
    ),
    e(
        "folder.needs_approval",
        Forms::single("Wasl stopped {folder} to check with you before a large change."),
        Forms::single("أوقف وصل {folder} ليسألك قبل إجراء تغيير كبير."),
        Some(Review),
        &[Folder],
        "A safety guard (many deletions, for example) stopped the run and waits for the person's yes.",
    ),
    e(
        "folder.conflicts",
        Forms::one_other(
            "{count} file in {folder} was changed in two places and needs your choice.",
            "{count} files in {folder} were changed in two places and need your choice.",
        ),
        Forms::six(
            "لا ملفات في {folder} تحتاج إلى اختيارك.",
            "ملف واحد في {folder} تغيّر في مكانين ويحتاج إلى اختيارك.",
            "ملفان في {folder} تغيّرا في مكانين ويحتاجان إلى اختيارك.",
            "{count} ملفات في {folder} تغيّرت في مكانين وتحتاج إلى اختيارك.",
            "{count} ملفًا في {folder} تغيّرت في مكانين وتحتاج إلى اختيارك.",
            "{count} ملف في {folder} تغيّرت في مكانين وتحتاج إلى اختيارك.",
        ),
        Some(Review),
        &[Folder, Count],
        "Open conflicts wait for the person. {count} is the number of files.",
    ),
    // ---- a run that finished but not every item synced
    e(
        "run.partial",
        Forms::one_other(
            "{count} item in {folder} didn't sync. Wasl will try again at the next sync.",
            "{count} items in {folder} didn't sync. Wasl will try again at the next sync.",
        ),
        Forms::six(
            "لا عناصر في {folder} لم تتم مزامنتها. سيحاول وصل مجددًا في المزامنة التالية.",
            "عنصر واحد في {folder} لم تتم مزامنته. سيحاول وصل مجددًا في المزامنة التالية.",
            "عنصران في {folder} لم تتم مزامنتهما. سيحاول وصل مجددًا في المزامنة التالية.",
            "{count} عناصر في {folder} لم تتم مزامنتها. سيحاول وصل مجددًا في المزامنة التالية.",
            "{count} عنصرًا في {folder} لم تتم مزامنتها. سيحاول وصل مجددًا في المزامنة التالية.",
            "{count} عنصر في {folder} لم تتم مزامنتها. سيحاول وصل مجددًا في المزامنة التالية.",
        ),
        Some(Retry),
        &[Folder, Count],
        "Shown after a run that finished with some items failing. {count} is how many items failed; Details has the first cause.",
    ),
    e(
        "run.partial_some",
        Forms::single("Wasl couldn't sync everything in {folder}. It will try again at the next sync."),
        Forms::single("تعذّر على وصل مزامنة كل شيء في {folder}. سيحاول مجددًا في المزامنة التالية."),
        Some(Retry),
        &[Folder],
        "Shown after a run that finished with some items failing when the message doesn't say how many (one failing item, or an older engine). Details has the first cause.",
    ),
    // ---- errors a run can end with (one per coarse error kind)
    e(
        "error.auth",
        Forms::single("Your Google sign-in has ended, so {folder} can't sync."),
        Forms::single("انتهى تسجيل دخولك إلى Google، لذلك لا يمكن مزامنة {folder}."),
        Some(SignInAgain),
        &[Folder],
        "Shown on a folder or in a notification when Google no longer accepts the saved sign-in.",
    ),
    e(
        "error.permission",
        Forms::single("Wasl isn't allowed to use {folder} any more, so it stopped syncing there."),
        Forms::single("لم يعد وصل مسموحًا له باستخدام {folder}، لذلك توقفت المزامنة هناك."),
        Some(OpenFolder),
        &[Folder],
        "The folder or the account refused Wasl access; the run stops (a fatal error). {folder} is the synced folder.",
    ),
    e(
        "error.quota",
        Forms::single("Google Drive is full, so {folder} can't send more files."),
        Forms::single("مساحة Google Drive ممتلئة، لذلك لا يستطيع {folder} رفع المزيد من الملفات."),
        Some(OpenDriveStorage),
        &[Folder],
        "The person's Drive storage is full (not this device's disk).",
    ),
    e(
        "error.not_found",
        Forms::single("Wasl couldn't find something it needed for {folder}."),
        Forms::single("تعذّر على وصل العثور على شيء يحتاجه لـ {folder}."),
        Some(Retry),
        &[Folder],
        "Something the run needed (a Drive folder, an upload) is gone. Not one vanished file: that is error.file_gone.",
    ),
    e(
        "error.locked",
        Forms::single(
            "Wasl can't sync {folder}: it is encrypted and locked on this device. Unlock it, then try again.",
        ),
        Forms::single(
            "لا يستطيع وصل مزامنة {folder}: فهو مشفّر ومقفل على هذا الجهاز. افتح قفله ثم حاول مجددًا.",
        ),
        Some(UnlockVault),
        &[Folder],
        "A vault folder whose keys this device doesn't hold (locked, or its keys unusable). Not a file held open by another program.",
    ),
    e(
        "error.network",
        Forms::single("Wasl can't reach Google Drive right now. It will try again at the next sync."),
        Forms::single("لا يستطيع وصل الوصول إلى Google Drive الآن. سيحاول مجددًا في المزامنة التالية."),
        Some(Retry),
        &[],
        "A run ended because Google Drive couldn't be reached. The only promise made is the next sync.",
    ),
    e(
        "error.rate_limit",
        Forms::single("Google Drive asked Wasl to slow down. It will try again in a moment."),
        Forms::single("طلب Google Drive من وصل أن يبطئ. سيحاول مجددًا بعد قليل."),
        Some(Retry),
        &[],
        "Google answered with 'too many requests'.",
    ),
    e(
        "error.server",
        Forms::single("Google Drive had a problem on its side. Wasl will try again at the next sync."),
        Forms::single("واجه Google Drive مشكلة من جانبه. سيحاول وصل مجددًا في المزامنة التالية."),
        Some(Retry),
        &[],
        "Google answered with a server error.",
    ),
    e(
        "error.io",
        Forms::single("Wasl couldn't read or write a file in {folder}. It will try again at the next sync."),
        Forms::single("تعذّر على وصل قراءة ملف في {folder} أو الكتابة فيه. سيحاول مجددًا في المزامنة التالية."),
        Some(OpenFolder),
        &[Folder],
        "A local file operation failed for a reason Wasl can't name more exactly.",
    ),
    e(
        "error.integrity",
        Forms::single(
            "A file in {folder} didn't arrive intact, so Wasl didn't keep it. It will be tried again.",
        ),
        Forms::single("لم يصل ملف في {folder} سليمًا، لذلك لم يحتفظ به وصل. ستتم محاولته مجددًا."),
        Some(Retry),
        &[Folder],
        "A downloaded or received file failed its check.",
    ),
    // ---- errors the daemon answers a request with
    e(
        "request.not_found",
        Forms::single("Wasl couldn't find what you asked for."),
        Forms::single("تعذّر على وصل العثور على ما طلبته."),
        None,
        &[],
        "The engine has no folder, device, file or vault by that name. Details carries the engine's own message.",
    ),
    e(
        "request.auth",
        Forms::single("Wasl isn't signed in to that Google account, or the sign-in has ended."),
        Forms::single("وصل غير مسجّل الدخول إلى حساب Google هذا، أو انتهى تسجيل الدخول."),
        Some(SignInAgain),
        &[],
        "A request needed a Google sign-in the engine doesn't have. Details carries the engine's own message.",
    ),
    e(
        "request.network",
        Forms::single("Wasl couldn't reach Google Drive for that. Check the connection and try again."),
        Forms::single("تعذّر على وصل الوصول إلى Google Drive لتنفيذ ذلك. تحقق من الاتصال وحاول مجددًا."),
        Some(Retry),
        &[],
        "A request needed Google Drive and the network failed. Details carries the engine's own message.",
    ),
    e(
        "request.other",
        Forms::single("Something went wrong with that request."),
        Forms::single("حدث خطأ أثناء تنفيذ هذا الطلب."),
        Some(Retry),
        &[],
        "Catch-all for a request failure the catalogue can't name; each one is logged. Details carries the engine's own message.",
    ),
    e(
        "request.permission",
        Forms::single("Wasl isn't allowed to do that."),
        Forms::single("لا يملك وصل الإذن لتنفيذ ذلك."),
        None,
        &[],
        "The engine refused a request the person isn't allowed to make. Details carries the engine's own message.",
    ),
    e(
        "request.locked",
        Forms::single("That is locked on this device. Unlock it, then try again."),
        Forms::single("هذا مقفل على هذا الجهاز. افتح قفله ثم حاول مجددًا."),
        Some(UnlockVault),
        &[],
        "A vault, or a view of one, is locked on this device so the engine can't do the request. Details carries the engine's message.",
    ),
    e(
        "error.file_gone",
        Forms::single("{file} was gone before Wasl could sync it, so it was skipped."),
        Forms::single("اختفى {file} قبل أن يتمكن وصل من مزامنته، لذلك تم تخطيه."),
        Some(Retry),
        &[File],
        "A file was deleted or moved between Wasl listing it and reading it. Not a problem; the next sync is clean. Not raised yet: the engine records a vanished file as a plain-text SKIP event, so no error carries this key today.",
    ),
    e(
        "error.disk_full",
        Forms::single("This device's storage is full, so Wasl couldn't save {file} in {folder}."),
        Forms::single("مساحة التخزين في هذا الجهاز ممتلئة، لذلك تعذّر على وصل حفظ {file} في {folder}."),
        Some(OpenStorageSettings),
        &[File, Folder],
        "The disk or phone storage on THIS device is full (contrast error.quota: the person's Drive is full).",
    ),
    e(
        "error.disk_quota",
        Forms::single(
            "The storage limit set for you on this device is reached, so Wasl couldn't save {file} in {folder}. Free some space, or ask whoever set the limit.",
        ),
        Forms::single(
            "تم بلوغ حدّ التخزين المحدد لك على هذا الجهاز، لذلك تعذّر على وصل حفظ {file} في {folder}. حرّر بعض المساحة أو اطلب من الجهة التي حدّدت الحدّ رفعه.",
        ),
        None,
        &[File, Folder],
        "A per-user disk quota on THIS device is used up (EDQUOT), though the disk itself may have room. Not error.disk_full (the device is full) and not error.quota (the person's Drive is full).",
    ),
    e(
        "error.name_refused",
        Forms::single("Drive or this device won't accept the name of {file}, so it was skipped."),
        Forms::single("لا يقبل Drive أو هذا الجهاز اسم {file}، لذلك تم تخطيه."),
        Some(OpenToRename),
        &[File],
        "The file name is too long, has a character or a reserved name that one side refuses. Wasl never renames it.",
    ),
    e(
        "error.invalid",
        Forms::single("Wasl couldn't do that: the request wasn't valid."),
        Forms::single("تعذّر على وصل تنفيذ ذلك: الطلب غير صالح."),
        None,
        &[],
        "Shown for a request the engine refused; Details carries the engine's own message.",
    ),
    e(
        "error.busy",
        Forms::single("Wasl is busy with that folder right now. Try again in a moment."),
        Forms::single("وصل مشغول بهذا المجلد الآن. حاول مجددًا بعد قليل."),
        Some(Retry),
        &[],
        "The folder is in the middle of a sync or another change.",
    ),
    e(
        "error.unsupported",
        Forms::single("This version of Wasl can't do that yet."),
        Forms::single("هذا الإصدار من وصل لا يستطيع تنفيذ ذلك بعد."),
        Some(UpdateWasl),
        &[],
        "The engine doesn't support the request (usually an older engine).",
    ),
    e(
        FALLBACK,
        Forms::single("Something went wrong syncing {folder}. Wasl will try again at the next sync."),
        Forms::single("حدث خطأ أثناء مزامنة {folder}. سيحاول وصل مجددًا في المزامنة التالية."),
        Some(Retry),
        &[Folder],
        "Fallback for any error the catalogue doesn't know. Always has Details; each unknown error is logged.",
    ),
    // ---- reaching the engine (the CLI and apps talk to a background program)
    e(
        "daemon.not_running",
        Forms::single("Wasl isn't running. Start it with: wasl daemon start"),
        Forms::single("وصل لا يعمل. شغّله بالأمر: wasl daemon start"),
        None,
        &[],
        "The command line couldn't find the background program. The command stays in English.",
    ),
    e(
        "daemon.untrusted",
        Forms::single("The program answering is not this user's Wasl."),
        Forms::single("البرنامج الذي يجيب ليس وصل الخاص بهذا المستخدم."),
        None,
        &[],
        "Whatever answers on Wasl's local address didn't prove it is this user's Wasl.",
    ),
    e(
        "daemon.client_needs_update",
        Forms::single(
            "This Wasl is older than the one running in the background, so they can't talk to each other. Update Wasl.",
        ),
        Forms::single("هذا الإصدار من وصل أقدم من الذي يعمل في الخلفية، لذلك لا يستطيعان التخاطب. حدّث وصل."),
        Some(UpdateWasl),
        &[],
        "The command line or app speaks only older protocol versions than the background program. Details says which side is behind.",
    ),
    e(
        "daemon.daemon_needs_update",
        Forms::single(
            "The Wasl running in the background is older than this one, so they can't talk to each other. Update Wasl and start it again.",
        ),
        Forms::single(
            "وصل العامل في الخلفية أقدم من هذا الإصدار، لذلك لا يستطيعان التخاطب. حدّث وصل وأعد تشغيله.",
        ),
        Some(UpdateWasl),
        &[],
        "The background program speaks only older protocol versions than the command line or app. Details says which side is behind.",
    ),
    e(
        "daemon.connection_lost",
        Forms::single("Lost the connection to Wasl. Try again."),
        Forms::single("انقطع الاتصال بوصل. حاول مجددًا."),
        Some(Retry),
        &[],
        "The link to the background program broke in the middle of a request.",
    ),
];

/// The entry a coarse run error kind maps to.
pub fn key_for_kind(kind: crate::run::ErrorKind) -> &'static str {
    use crate::run::ErrorKind as K;
    match kind {
        K::Auth => "error.auth",
        K::Permission => "error.permission",
        K::Quota => "error.quota",
        K::NotFound => "error.not_found",
        K::Locked => "error.locked",
        K::Network => "error.network",
        K::RateLimit => "error.rate_limit",
        K::Server => "error.server",
        K::Io => "error.io",
        K::Integrity => "error.integrity",
        K::Other => FALLBACK,
    }
}

/// The operating system an error message came from: the same `(os error N)` number means different
/// things on each (28 is "no space" on Linux and macOS, "out of paper" on Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    /// Linux and Android.
    Linux,
    /// macOS and iOS.
    Apple,
}

impl Platform {
    /// The system this program runs on, which is where a local engine's I/O errors come from.
    pub fn current() -> Platform {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(any(target_os = "macos", target_os = "ios")) {
            Platform::Apple
        } else {
            Platform::Linux
        }
    }

    /// The error numbers for a full disk, for a used-up quota, and for a name too long or not valid.
    fn tables(self) -> (&'static [i32], &'static [i32], &'static [i32]) {
        match self {
            // ERROR_HANDLE_DISK_FULL, ERROR_DISK_FULL; no per-user quota number; ERROR_INVALID_NAME,
            // ERROR_FILENAME_EXCED_RANGE.
            Platform::Windows => (&[39, 112], &[], &[123, 206]),
            // ENOSPC; EDQUOT; ENAMETOOLONG.
            Platform::Linux => (&[28], &[122], &[36]),
            // ENOSPC; EDQUOT; ENAMETOOLONG.
            Platform::Apple => (&[28], &[69], &[63]),
        }
    }
}

/// The most specific entry for a failure that came as a coarse `kind` and its raw cause, for callers
/// that hold nothing more typed (the CLI and apps read the daemon's reply). It only trusts the numbers
/// the operating system itself puts in an I/O message (`(os error 28)`), read with this system's table,
/// never words, so a file called "Disk full notes.txt" can't change the answer.
pub fn classify(kind: crate::run::ErrorKind, raw: &str) -> &'static str {
    classify_for(Platform::current(), kind, raw)
}

/// [`classify`] for errors that came from `platform`.
pub fn classify_for(platform: Platform, kind: crate::run::ErrorKind, raw: &str) -> &'static str {
    use crate::run::ErrorKind as K;
    if matches!(kind, K::Io | K::Other)
        && let Some(code) = os_error_code(raw)
    {
        let (disk_full, disk_quota, name_refused) = platform.tables();
        if disk_full.contains(&code) {
            return "error.disk_full";
        }
        if disk_quota.contains(&code) {
            return "error.disk_quota";
        }
        if name_refused.contains(&code) {
            return "error.name_refused";
        }
    }
    key_for_kind(kind)
}

/// What a run that finished with failing items says in `RunOutcome::message`: the first cause alone
/// for one item, and a count in front for several. [`partial_count`] reads the count back.
pub fn partial_message(errors: u64, first: &str) -> String {
    match errors {
        1 => first.to_owned(),
        n => format!("{n} items couldn't sync. First: {first}"),
    }
}

/// How many items a partial run's message says did not sync; None when it carries no count (one
/// failing item is worded by its cause alone, and an older engine may word it any way), so nothing
/// claims a number the message doesn't hold.
pub fn partial_count(message: &str) -> Option<u64> {
    message
        .split_once(" items couldn't sync. First: ")
        .and_then(|(n, _)| n.parse().ok())
}

/// The number in a trailing `(os error N)`, as `std::io::Error` prints it.
pub fn os_error_code(raw: &str) -> Option<i32> {
    let i = raw.rfind("(os error ")?;
    let rest = &raw[i + "(os error ".len()..];
    let end = rest.find(')')?;
    rest[..end].parse().ok()
}

/// The entry with this key.
pub fn entry(key: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.key == key)
}

/// Words, fix and details for one entry in one language: what a screen draws and an agent reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Rendered {
    pub key: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
    #[serde(default, skip_serializing_if = "Values::is_empty")]
    pub values: Values,
    /// The raw cause, made safe by [`redact`]; shown collapsed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl Values {
    fn is_empty(&self) -> bool {
        *self == Values::default()
    }
}

fn stand_in(slot: Slot, lang: Lang) -> &'static str {
    match (slot, lang) {
        (Slot::Folder, Lang::En) => "this folder",
        (Slot::Folder, Lang::Ar) => "هذا المجلد",
        (Slot::File, Lang::En) => "a file",
        (Slot::File, Lang::Ar) => "ملف",
        (Slot::Device, Lang::En) => "another device",
        (Slot::Device, Lang::Ar) => "جهاز آخر",
        (Slot::Count, _) => "0",
    }
}

fn fill(template: &str, values: &Values, lang: Lang) -> String {
    let mut out = String::with_capacity(template.len() + 24);
    let mut rest = template;
    'scan: while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        for slot in [Slot::Folder, Slot::File, Slot::Device, Slot::Count] {
            let tag = format!("{{{}}}", slot.name());
            if after.starts_with(&tag) {
                let v = match slot {
                    Slot::Folder => values.folder.clone(),
                    Slot::File => values.file.clone(),
                    Slot::Device => values.device.clone(),
                    Slot::Count => values.count.map(|c| c.to_string()),
                };
                out.push_str(&v.unwrap_or_else(|| stand_in(slot, lang).to_owned()));
                rest = &after[tag.len()..];
                continue 'scan;
            }
        }
        out.push('{');
        rest = &after[1..];
    }
    out.push_str(rest);
    out
}

/// Renders `key` in `lang`. A key the catalogue doesn't have renders the fallback entry, keeps the
/// requested key out of the result's `key`, and calls `log` once with a line naming it and its
/// redacted cause, so it can be added to the catalogue. The two catch-all entries log the same way. `raw` is the underlying cause; it becomes Details after [`redact`].
pub fn render(
    key: &str,
    values: &Values,
    lang: Lang,
    raw: Option<&str>,
    secrets: &[&str],
    log: &mut dyn FnMut(String),
) -> Rendered {
    render_with(key, values, lang, raw, &[], secrets, log)
}

/// [`render`] for a caller that knows more than the one folder in `values`: every folder root and
/// secret it knows are taken out of the cause ([`redact_with`]).
pub fn render_with(
    key: &str,
    values: &Values,
    lang: Lang,
    raw: Option<&str>,
    roots: &[&str],
    secrets: &[&str],
    log: &mut dyn FnMut(String),
) -> Rendered {
    let redacted = |r: &str| {
        let mut all: Vec<&str> = values.folder_root().into_iter().collect();
        all.extend_from_slice(roots);
        redact_with(r, &all, secrets)
    };
    let ent = entry(key).unwrap_or_else(|| entry(FALLBACK).expect("the fallback entry exists"));
    if ent.key == FALLBACK || ent.key == REQUEST_FALLBACK {
        let cause = raw
            .map(redacted)
            .map(|c| c.chars().take(200).collect::<String>())
            .filter(|c| !c.is_empty());
        log(match cause {
            Some(c) => {
                format!("wasl: no specific catalogue entry for error {key:?} ({c}); showing the fallback")
            }
            None => format!("wasl: no specific catalogue entry for error {key:?}; showing the fallback"),
        });
    }
    let forms = match lang {
        Lang::En => &ent.en,
        Lang::Ar => &ent.ar,
    };
    // A count the caller didn't give reads as zero, in the language's own form for none.
    let cat = plural(lang, values.count.unwrap_or(0));
    let text = fill(forms.pick(cat), values, lang);
    let details = raw.map(redacted).filter(|d| !d.is_empty());
    Rendered {
        key: ent.key.to_owned(),
        text,
        fix: ent.fix,
        values: values.clone(),
        details,
    }
}

impl Values {
    /// The folder's own path, when the caller put one in `folder`; `redact` shortens paths to it.
    fn folder_root(&self) -> Option<&str> {
        self.folder
            .as_deref()
            .filter(|f| f.contains('/') || f.contains('\\'))
    }
}

/// Makes a raw cause safe to show and to copy. Tokens, sign-in codes, `Bearer`/`Basic` credentials,
/// secret-looking `key=value` and JSON pairs, e-mail addresses and every one of `secrets` (account ids
/// and the like) become `[hidden]`. A path inside `folder_root` is cut to `Folder/…/file`; any other
/// absolute path becomes `…`, spaces in it included. The whole text is scanned, not word by word.
pub fn redact(raw: &str, folder_root: Option<&str>, secrets: &[&str]) -> String {
    redact_with(raw, &folder_root.into_iter().collect::<Vec<_>>(), secrets)
}

/// [`redact`] for a caller that knows several folder roots (every pair's folder): a path inside any of
/// them is cut to that folder's own name.
pub fn redact_with(raw: &str, roots: &[&str], secrets: &[&str]) -> String {
    use regex::{Captures, Regex};
    use std::sync::OnceLock;
    struct Rules {
        credential: Regex,
        pair: Regex,
        token: Regex,
        email: Regex,
        marked: Regex,
        windows_path: Regex,
        unix_path: Regex,
    }
    static RULES: OnceLock<Rules> = OnceLock::new();
    let r = RULES.get_or_init(|| {
        // A path runs to the next whitespace; a space stays inside it only when the next piece holds
        // a separator too (`Jane Doe\x.txt`), so words after a path ("isn't in a view") stay.
        let chunk = r#"[^\s'"<>|*?]*"#;
        let tail = format!(r#"{chunk}(?: {chunk}[\\/]{chunk})*"#);
        let re = |p: &str| Regex::new(p).expect("a fixed pattern");
        Rules {
            credential: re(r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]+"),
            pair: re(
                r#"(?i)\b("?(?:[\w-]*(?:access[_-]?token|refresh[_-]?token|id[_-]?token|token|client[_-]?secret|password|passphrase|access[_-]?key(?:[_-]?id)?|api[_-]?key|private[_-]?key|secret(?:[_-]?access[_-]?key)?|authorization|cookie|session[_-]?id|account[_-]?id|permission[_-]?id|email(?:[_-]?address)?|display[_-]?name|user(?:[_-]?(?:name|id))?)|code)"?\s*[:=]\s*)("[^"]*"|'[^']*'|[^\s&,;}]+)"#,
            ),
            token: re(r"(?i)(?:ya29\.|1//|eyJ|gho_|ghp_|sk-|GOCSPX-)[A-Za-z0-9._\-/]+"),
            email: re(r"[\w.+-]+@[\w-]+(?:\.[\w-]+)+"),
            marked: re(&format!("\u{2}R(\\d+)\u{2}({tail})")),
            // The drive letter must not sit inside a word: `s:/` in `https://` is not a drive.
            windows_path: re(&format!(r#"(^|[^A-Za-z0-9])([A-Za-z]:[\\/]{tail})"#)),
            unix_path: re(&format!(r#"(^|[\s'"(=])(/[^\s/]{tail})"#)),
        }
    });
    let mut text = raw.to_owned();
    for s in secrets.iter().filter(|s| !s.is_empty()) {
        text = text.replace(s, "[hidden]");
    }
    text = r.credential.replace_all(&text, "$1 [hidden]").into_owned();
    text = r.pair.replace_all(&text, "${1}[hidden]").into_owned();
    text = r.token.replace_all(&text, "[hidden]").into_owned();
    text = r.email.replace_all(&text, "[hidden]").into_owned();
    // Longest first, so a folder inside another is cut to its own name; each root counts once.
    let mut roots: Vec<&str> = roots
        .iter()
        .map(|f| f.trim_end_matches(['/', '\\']))
        .filter(|f| !f.is_empty())
        .collect();
    roots.sort_by_key(|f| std::cmp::Reverse(f.len()));
    roots.dedup();
    let mut names: Vec<&str> = Vec::new();
    for (i, root) in roots.iter().enumerate() {
        let variants: Vec<String> = [root.to_string(), root.replace('\\', "/"), root.replace('/', "\\")]
            .iter()
            .map(|v| regex::escape(v))
            .collect();
        // The root counts only as a whole name: `/a/Life` is not the start of `/a/LifeOS2/x`.
        let inside =
            Regex::new(&format!(r"(?:{})([\\/]|[^\w\\/.-]|$)", variants.join("|"))).expect("a built pattern");
        let mark = format!("\u{2}R{i}\u{2}${{1}}");
        text = inside.replace_all(&text, mark.as_str()).into_owned();
        names.push(root.rsplit(['/', '\\']).next().unwrap_or(root));
    }
    text = r
        .marked
        .replace_all(&text, |c: &Captures| {
            let name = c[1]
                .parse::<usize>()
                .ok()
                .and_then(|i| names.get(i).copied())
                .unwrap_or("…");
            let (body, rest) = split_punct(&c[2]);
            let parts: Vec<&str> = body.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
            let short = match parts.as_slice() {
                [] => name.to_owned(),
                [one] => format!("{name}/{one}"),
                [.., last] => format!("{name}/…/{last}"),
            };
            format!("{short}{rest}")
        })
        .into_owned();
    text = r
        .windows_path
        .replace_all(&text, |c: &Captures| {
            format!("{}…{}", &c[1], split_punct(&c[2]).1)
        })
        .into_owned();
    text = r
        .unix_path
        .replace_all(&text, |c: &Captures| {
            format!("{}…{}", &c[1], split_punct(&c[2]).1)
        })
        .into_owned();
    text
}

/// Splits trailing sentence punctuation off a matched path: `x.txt:` is `x.txt` and `:`.
fn split_punct(path: &str) -> (&str, &str) {
    let cut = path.trim_end_matches([':', ',', ';', '.', ')']).len();
    path.split_at(cut)
}

/// The reference document: one section per entry, generated from [`ENTRIES`].
pub fn reference_markdown() -> String {
    let mut s = String::from(
        "# Status and error words\n\n\
         Generated from `engine/crates/wasl-proto/src/words.rs`; do not edit by hand. Run\n\
         `WASL_BLESS=1 cargo test -p wasl-proto words` after changing the catalogue.\n\n\
         Every status and error Wasl shows is one of these entries. The key is stable: a script or an\n\
         agent acts on the key, the typed values and the fix, never on the sentence. Values in the words\n\
         are written `{name}`; the engine fills them and renders the entry in the caller's language,\n\
         falling back to English.\n\n",
    );
    for ent in ENTRIES {
        s.push_str(&format!("## `{}`\n\n", ent.key));
        s.push_str(&format!("- Note: {}\n", ent.note));
        let slots: Vec<&str> = ent.slots.iter().map(|x| x.name()).collect();
        s.push_str(&format!(
            "- Values: {}\n",
            if slots.is_empty() {
                "none".into()
            } else {
                slots.join(", ")
            }
        ));
        s.push_str(&format!(
            "- Fix: {}\n",
            ent.fix
                .map_or("none".to_owned(), |f| format!("`{}`", fix_name(f)))
        ));
        for (lang, forms) in [("English", &ent.en), ("Arabic", &ent.ar)] {
            for (cat, text) in [
                ("zero", forms.zero),
                ("one", forms.one),
                ("two", forms.two),
                ("few", forms.few),
                ("many", forms.many),
                ("other", Some(forms.other)),
            ] {
                if let Some(t) = text {
                    if ent.slots.contains(&Slot::Count) {
                        s.push_str(&format!("- {lang} ({cat}): {t}\n"));
                    } else {
                        s.push_str(&format!("- {lang}: {t}\n"));
                    }
                }
            }
        }
        s.push('\n');
    }
    s
}

fn fix_name(f: Fix) -> String {
    serde_json::to_value(f)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> impl FnMut(String) {
        |_| {}
    }

    #[test]
    fn every_entry_is_complete() {
        let mut seen = std::collections::HashSet::new();
        for ent in ENTRIES {
            assert!(seen.insert(ent.key), "duplicate key {}", ent.key);
            assert!(
                !ent.note.trim().is_empty(),
                "{} has no translator's note",
                ent.key
            );
            for (lang, forms) in [("en", &ent.en), ("ar", &ent.ar)] {
                assert!(!forms.other.trim().is_empty(), "{} lacks {lang} text", ent.key);
                for t in forms.all() {
                    assert!(!t.trim().is_empty(), "{} has an empty {lang} form", ent.key);
                    for slot in [Slot::Folder, Slot::File, Slot::Device, Slot::Count] {
                        if t.contains(&format!("{{{}}}", slot.name())) {
                            assert!(
                                ent.slots.contains(&slot),
                                "{} {lang} uses {{{}}} without declaring it",
                                ent.key,
                                slot.name()
                            );
                        }
                    }
                }
            }
            let plural_entry = ent.slots.contains(&Slot::Count);
            if plural_entry {
                assert!(ent.en.one.is_some(), "{} needs an English 'one' form", ent.key);
                let a = &ent.ar;
                assert!(
                    a.zero.is_some()
                        && a.one.is_some()
                        && a.two.is_some()
                        && a.few.is_some()
                        && a.many.is_some(),
                    "{} needs all six Arabic forms",
                    ent.key
                );
            } else {
                let forms = [&ent.en, &ent.ar];
                assert!(
                    forms.iter().all(|f| f.zero.is_none()
                        && f.one.is_none()
                        && f.two.is_none()
                        && f.few.is_none()
                        && f.many.is_none()),
                    "{} has plural forms but takes no count",
                    ent.key
                );
            }
        }
    }

    #[test]
    fn plurals_in_english_and_arabic() {
        let r = |n, lang| {
            render(
                "status.folders_need_you",
                &Values::default().count(n),
                lang,
                None,
                &[],
                &mut quiet(),
            )
            .text
        };
        assert_eq!(r(1, Lang::En), "1 folder needs you");
        assert_eq!(r(2, Lang::En), "2 folders need you");
        assert_eq!(r(0, Lang::Ar), "لا مجلدات تحتاج إليك");
        assert_eq!(r(1, Lang::Ar), "مجلد واحد يحتاج إليك");
        assert_eq!(r(2, Lang::Ar), "مجلدان يحتاجان إليك");
        assert_eq!(r(3, Lang::Ar), "3 مجلدات تحتاج إليك");
        assert_eq!(r(11, Lang::Ar), "11 مجلدًا يحتاج إليك");
        assert_eq!(r(100, Lang::Ar), "100 مجلد يحتاج إليك");
        assert_eq!(r(102, Lang::Ar), "102 مجلد يحتاج إليك");
    }

    #[test]
    fn every_run_error_kind_has_its_entry() {
        use crate::run::ErrorKind as K;
        let want = [
            (K::Auth, "error.auth"),
            (K::Permission, "error.permission"),
            (K::Quota, "error.quota"),
            (K::NotFound, "error.not_found"),
            (K::Locked, "error.locked"),
            (K::Network, "error.network"),
            (K::RateLimit, "error.rate_limit"),
            (K::Server, "error.server"),
            (K::Io, "error.io"),
            (K::Integrity, "error.integrity"),
            (K::Other, FALLBACK),
        ];
        for (kind, key) in want {
            assert_eq!(key_for_kind(kind), key);
            assert!(entry(key).is_some(), "{key} is not in the catalogue");
        }
    }

    #[test]
    fn no_entry_shows_a_bare_internal_code() {
        for ent in ENTRIES {
            for t in ent.en.all().chain(ent.ar.all()) {
                let bare = t
                    .split(|c: char| c.is_whitespace() || "{}.,:'\"".contains(c))
                    .any(|w| w.contains('_') || (w.len() > 2 && w.chars().all(|c| c.is_ascii_uppercase())));
                assert!(!bare, "{} shows a bare code: {t}", ent.key);
            }
        }
    }

    #[test]
    fn missing_language_falls_back_to_english() {
        assert_eq!(Lang::parse("fr_FR"), None);
        assert_eq!(Lang::parse("ar-EG"), Some(Lang::Ar));
    }

    #[test]
    fn an_unknown_key_or_an_other_error_gets_the_fallback_and_one_log_line() {
        let mut lines = Vec::new();
        let r = render(
            "error.never_heard_of_it",
            &Values::default().folder("LifeOS"),
            Lang::En,
            Some("boom"),
            &[],
            &mut |l| lines.push(l),
        );
        assert_eq!(r.key, FALLBACK);
        assert_eq!(
            r.text,
            "Something went wrong syncing LifeOS. Wasl will try again at the next sync."
        );
        assert_eq!(r.fix, Some(Fix::Retry));
        assert_eq!(r.details.as_deref(), Some("boom"));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("error.never_heard_of_it"));
        // An error kind the engine itself calls Other reaches the fallback by key, and logs too.
        let mut other = Vec::new();
        let r = render(
            key_for_kind(crate::run::ErrorKind::Other),
            &Values::default(),
            Lang::En,
            None,
            &[],
            &mut |l| other.push(l),
        );
        assert_eq!(r.key, FALLBACK);
        assert_eq!(other.len(), 1);
        let mut known = Vec::new();
        render(
            "error.network",
            &Values::default(),
            Lang::En,
            None,
            &[],
            &mut |l| known.push(l),
        );
        assert!(known.is_empty(), "a known key logs nothing");
    }

    #[test]
    fn a_missing_value_reads_plainly_and_a_value_is_not_filled_twice() {
        let r = render(
            "error.disk_full",
            &Values::default(),
            Lang::En,
            None,
            &[],
            &mut quiet(),
        );
        assert_eq!(
            r.text,
            "This device's storage is full, so Wasl couldn't save a file in this folder."
        );
        let r = render(
            "error.disk_full",
            &Values::default().file("{folder}").folder("Docs"),
            Lang::En,
            None,
            &[],
            &mut quiet(),
        );
        assert_eq!(
            r.text,
            "This device's storage is full, so Wasl couldn't save {folder} in Docs."
        );
    }

    #[test]
    fn only_the_operating_systems_error_numbers_pick_the_specific_entries() {
        use crate::run::ErrorKind as K;
        use Platform::*;
        // The same number means different things on each system: 28 is "no space" on Linux and macOS
        // but "out of paper" on Windows; 63 is "name too long" on macOS only; 39 is "disk full" on
        // Windows but "directory not empty" on Linux.
        let cases = [
            (
                Linux,
                K::Io,
                "No space left on device (os error 28)",
                "error.disk_full",
            ),
            (
                Apple,
                K::Io,
                "No space left on device (os error 28)",
                "error.disk_full",
            ),
            (Windows, K::Io, "Out of paper. (os error 28)", "error.io"),
            (
                Windows,
                K::Io,
                "There is not enough space on the disk. (os error 112)",
                "error.disk_full",
            ),
            (
                Windows,
                K::Io,
                "The disk is full. (os error 39)",
                "error.disk_full",
            ),
            (Linux, K::Io, "Directory not empty (os error 39)", "error.io"),
            (Linux, K::Io, "boom (os error 112)", "error.io"),
            (
                Linux,
                K::Io,
                "Disk quota exceeded (os error 122)",
                "error.disk_quota",
            ),
            (
                Apple,
                K::Io,
                "Disk quota exceeded (os error 69)",
                "error.disk_quota",
            ),
            (Apple, K::Io, "boom (os error 122)", "error.io"),
            (
                Windows,
                K::Io,
                "The filename or extension is too long. (os error 206)",
                "error.name_refused",
            ),
            (
                Windows,
                K::Io,
                "The filename is not valid. (os error 123)",
                "error.name_refused",
            ),
            (Windows, K::Io, "File name too long (os error 36)", "error.io"),
            (
                Linux,
                K::Io,
                "File name too long (os error 36)",
                "error.name_refused",
            ),
            (Linux, K::Io, "boom (os error 63)", "error.io"),
            (Linux, K::Io, "boom (os error 123)", "error.io"),
            (
                Apple,
                K::Io,
                "File name too long (os error 63)",
                "error.name_refused",
            ),
            (Apple, K::Io, "boom (os error 36)", "error.io"),
            (Windows, K::Io, "boom (os error 281)", "error.io"),
            // A refused permission stays a permission problem, whatever number it carries.
            (
                Linux,
                K::Permission,
                "File name too long (os error 36)",
                "error.permission",
            ),
            (
                Windows,
                K::Permission,
                "access denied (os error 123)",
                "error.permission",
            ),
            // Drive's quota is not this device's disk.
            (Linux, K::Quota, "quota (os error 28)", "error.quota"),
            (
                Linux,
                K::Permission,
                "Disk full notes.txt: access denied (os error 5)",
                "error.permission",
            ),
            (
                Linux,
                K::Auth,
                "invalid_grant: token has been expired or revoked",
                "error.auth",
            ),
            (
                Linux,
                K::NotFound,
                "This pair has no Drive folder",
                "error.not_found",
            ),
            (Linux, K::Network, "dns error", "error.network"),
        ];
        for (platform, kind, raw, key) in cases {
            assert_eq!(classify_for(platform, kind, raw), key, "{platform:?} {raw}");
            assert!(entry(key).is_some());
        }
        // The caller's own platform is the default.
        assert_eq!(
            classify(K::Io, "boom (os error 281)"),
            classify_for(Platform::current(), K::Io, "boom (os error 281)")
        );
        let fix = |k| entry(k).unwrap().fix;
        assert_eq!(fix("error.disk_full"), Some(Fix::OpenStorageSettings));
        assert_eq!(fix("error.quota"), Some(Fix::OpenDriveStorage));
        assert_eq!(fix("error.name_refused"), Some(Fix::OpenToRename));
    }

    #[test]
    fn a_locked_entry_speaks_of_a_locked_vault_not_a_busy_file() {
        // The engine's Locked is a vault whose keys this device doesn't hold (wasld vaults.rs, ffi).
        for key in ["error.locked", "request.locked"] {
            let ent = entry(key).unwrap();
            assert_eq!(ent.fix, Some(Fix::UnlockVault), "{key}");
            for t in ent.en.all() {
                assert!(t.contains("locked"), "{key}: {t}");
                assert!(
                    !t.contains("another program") && !t.contains("in use"),
                    "{key}: {t}"
                );
            }
            for t in ent.ar.all() {
                assert!(t.contains("مقفل"), "{key}: {t}");
            }
        }
    }

    #[test]
    fn a_request_has_its_own_words_for_every_kind() {
        // A request the person made (a CLI command) never reads as a sync: no "sync", no folder.
        for key in [
            "request.auth",
            "request.network",
            "request.permission",
            "request.not_found",
            "request.locked",
            "request.other",
        ] {
            let ent = entry(key).unwrap_or_else(|| panic!("{key} is not in the catalogue"));
            assert!(ent.slots.is_empty(), "{key} takes no values");
            for t in ent.en.all() {
                assert!(!t.contains("sync"), "{key}: {t}");
            }
        }
        assert_eq!(entry("request.auth").unwrap().fix, Some(Fix::SignInAgain));
        assert_eq!(entry("request.network").unwrap().fix, Some(Fix::Retry));
        // The catch-all for requests is an unknown error too: it logs, with the redacted cause.
        let mut lines = Vec::new();
        let r = render(
            "request.other",
            &Values::default(),
            Lang::En,
            Some("boom for user@example.com"),
            &[],
            &mut |l| lines.push(l),
        );
        assert_eq!(r.key, "request.other");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("boom for [hidden]"), "{lines:?}");
        assert!(!lines[0].contains("user@example.com"));
    }

    #[test]
    fn a_partial_run_says_how_many_items_did_not_sync() {
        assert_eq!(partial_message(1, "Photo a.jpg: failed"), "Photo a.jpg: failed");
        // One failing item carries no count in its message, so none can be read back.
        assert_eq!(partial_count(&partial_message(1, "Photo a.jpg: failed")), None);
        assert_eq!(partial_count(&partial_message(7, "Photo a.jpg: failed")), Some(7));
        assert_eq!(partial_count("some text with no count"), None);
        let r = |n, lang| {
            render(
                "run.partial",
                &Values::default().folder("Docs").count(n),
                lang,
                Some(&partial_message(n, "x")),
                &[],
                &mut quiet(),
            )
        };
        assert_eq!(
            r(1, Lang::En).text,
            "1 item in Docs didn't sync. Wasl will try again at the next sync."
        );
        assert_eq!(
            r(3, Lang::En).text,
            "3 items in Docs didn't sync. Wasl will try again at the next sync."
        );
        assert!(r(3, Lang::Ar).text.contains('3') && r(3, Lang::Ar).text.contains("Docs"));
        assert!(
            r(3, Lang::En)
                .details
                .unwrap()
                .starts_with("3 items couldn't sync")
        );
    }

    #[test]
    fn a_catch_all_logs_the_cause_it_could_not_name() {
        let mut lines = Vec::new();
        render(
            FALLBACK,
            &Values::default(),
            Lang::En,
            Some("weird failure at C:\\Users\\Jane Doe\\x.txt"),
            &[],
            &mut |l| lines.push(l),
        );
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("weird failure"), "{lines:?}");
        assert!(!lines[0].contains("Jane"), "{lines:?}");
    }

    #[test]
    fn no_entry_claims_what_it_cannot_know() {
        // After a failure the engine may have changed or kept other files, and a run reports only
        // its first cause, so no entry says nothing changed, nothing was lost, or the rest synced.
        for ent in ENTRIES {
            for t in ent.en.all().chain(ent.ar.all()) {
                let lower = t.to_lowercase();
                for claim in [
                    "nothing was changed",
                    "nothing was lost",
                    "the rest",
                    "everything else",
                    "لم يتغير شيء",
                    "لم يضع شيء",
                    "بقية",
                ] {
                    assert!(!lower.contains(claim), "{}: claims {claim:?}: {t}", ent.key);
                }
            }
        }
    }

    fn planted(raw: &str, root: Option<&str>) -> String {
        redact(raw, root, &["acct-7f3"])
    }

    #[test]
    fn details_never_hold_tokens_accounts_or_outside_paths() {
        let cases = [
            "request failed for Bearer ya29.A0planted-token on acct-7f3",
            r#"{"access_token":"ya29.abc","refresh_token":"opaque-refresh-xyz","client_secret":"GOCSPX-planted"}"#,
            "GET /cb?code=4/0AplantedCode&state=x failed",
            "Authorization: Basic dXNlcjpwbGFudGVkcGFzcw==",
            "sent to user@example.com",
            "failed to open C:\\Users\\Jane Doe\\Documents\\x.txt: denied",
            "cannot read /home/user/Secret Stuff/other.txt (os error 5)",
            "path=/home/user/Secret/other.txt",
            "key=acct-7f3",
        ];
        for raw in cases {
            let d = planted(raw, None);
            for leak in [
                "ya29",
                "opaque-refresh",
                "GOCSPX",
                "4/0Aplanted",
                "dXNlcjp",
                "user@example.com",
                "Jane",
                "Doe",
                "Secret",
                "acct-7f3",
                "C:\\Users",
                "/home/user",
            ] {
                assert!(!d.contains(leak), "{leak} leaked from {raw:?}: {d}");
            }
        }
        assert_eq!(
            planted("failed to open C:\\Users\\Jane Doe\\x.txt: denied", None),
            "failed to open …: denied"
        );
    }

    #[test]
    fn a_path_in_the_folder_is_cut_to_the_folder() {
        let d = redact(
            "cannot open C:\\Users\\a b\\LifeOS\\x\\y\\z.txt: denied",
            Some("C:\\Users\\a b\\LifeOS"),
            &[],
        );
        assert_eq!(d, "cannot open LifeOS/…/z.txt: denied");
        let d = redact(
            "bad '/home/user/LifeOS/a/b.txt' and /home/user/Other/c.txt (os error 2)",
            Some("/home/user/LifeOS"),
            &[],
        );
        assert_eq!(d, "bad 'LifeOS/…/b.txt' and … (os error 2)");
        // A sibling that only starts with the folder's name is outside the folder.
        let d = redact(
            "cannot open /home/user/LifeOS2/x/y.txt: denied",
            Some("/home/user/LifeOS"),
            &[],
        );
        assert_eq!(d, "cannot open …: denied");
    }

    #[test]
    fn a_secret_key_is_hidden_in_any_spelling() {
        // Google-style JSON and form bodies: camelCase, snake_case, any case, with or without quotes.
        let hidden = [
            r#"{"refreshToken":"1//0gOpaqueRefresh","accessToken":"ya29.A0Opaque","idToken":"abc.def.ghi"}"#,
            r#"{"access_token": "opaque-1", "refresh_token": "opaque-2", "token_type": "Bearer", "expires_in": 3599}"#,
            r#"{"clientSecret":"opaque-3","client_secret":"opaque-4","apiKey":"opaque-5","api_key":"opaque-6"}"#,
            r#"{"accountId":123456789,"account_id":"opaque-7","emailAddress":"someone@example.com"}"#,
            r#"{"displayName":"JaneDoe","permissionId":"opaque-8","userName":"opaque-9"}"#,
            "request failed: apiKey=opaque-10&alt=json",
            "accountId: 123456789 rejected",
            "REFRESHTOKEN=opaque-11 and Client-Secret: opaque-12",
            "x-api-key: opaque-13",
            "Set-Cookie: sessionId=opaque-14; Path=/",
            "privateKey: opaque-15",
        ];
        for raw in hidden {
            let d = redact(raw, None, &[]);
            for planted in raw
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '/'))
                .filter(|w| {
                    w.starts_with("opaque")
                        || w.starts_with("ya29")
                        || w.starts_with("1//")
                        || *w == "123456789"
                        || *w == "abc.def.ghi"
                        || w.starts_with("someone")
                        || *w == "JaneDoe"
                })
            {
                assert!(!d.contains(planted), "{planted} leaked from {raw:?}: {d}");
            }
            assert!(d.contains("[hidden]"), "{raw:?}: {d}");
        }
        // What is not a secret stays readable.
        let d = redact(
            r#"{"token_type":"Bearer","expires_in":3599,"scope":"drive"}"#,
            None,
            &[],
        );
        assert!(
            d.contains("expires_in") && d.contains("3599") && d.contains("drive"),
            "{d}"
        );
        assert_eq!(
            redact("zipCode: 12345, errorCode: 5", None, &[]),
            "zipCode: 12345, errorCode: 5"
        );
    }

    #[test]
    fn a_url_is_not_read_as_a_windows_path() {
        // `s:/` in `https://` is not a drive letter: no boundary before it, no path.
        for raw in [
            "GET https://www.googleapis.com/drive/v3/files failed: 403",
            "the redirect to http://localhost:8080/cb failed",
            "see (https://example.com/a/b?x=1) and ftp://host/dir/file.txt",
        ] {
            assert_eq!(redact(raw, None, &[]), raw);
        }
        // A real drive path is still cut, at the start, after a space and after a quote.
        assert_eq!(redact(r"C:\Users\user\x.txt: denied", None, &[]), "…: denied");
        assert_eq!(
            redact(r"cannot open C:\Users\user\x.txt: denied", None, &[]),
            "cannot open …: denied"
        );
        assert_eq!(
            redact(r#"open "D:/a/b c/d.txt" failed"#, None, &[]),
            r#"open "…" failed"#
        );
    }

    #[test]
    fn a_secret_key_with_a_prefix_is_hidden_and_a_code_is_not_a_secret() {
        for (raw, planted) in [
            ("oauth_token=opaque-1&x=1", "opaque-1"),
            ("session_token: opaque-2", "opaque-2"),
            ("userAccountId=opaque-3", "opaque-3"),
            ("appSecret=opaque-4", "opaque-4"),
            ("AWS_SECRET_ACCESS_KEY=opaque-5", "opaque-5"),
            ("AWS_ACCESS_KEY_ID=opaque-5b", "opaque-5b"),
            (r#"{"oauth_token":"opaque-6","appSecret":"opaque-7"}"#, "opaque-6"),
            (r#"{"oauth_token":"opaque-6","appSecret":"opaque-7"}"#, "opaque-7"),
            ("x-session-id: opaque-8", "opaque-8"),
            ("userEmail=opaque-9", "opaque-9"),
        ] {
            let d = redact(raw, None, &[]);
            assert!(!d.contains(planted), "{planted} leaked from {raw:?}: {d}");
            assert!(d.contains("[hidden]"), "{raw:?}: {d}");
        }
        // The key stays; only the value goes.
        assert_eq!(
            redact("AWS_SECRET_ACCESS_KEY=opaque-5", None, &[]),
            "AWS_SECRET_ACCESS_KEY=[hidden]"
        );
        // A prefixed `code` is a status, not a secret.
        for raw in [
            "zipCode=94107 errorCode: 404",
            "error_code=17, statusCode=500",
            "ZIP_CODE: 94107",
        ] {
            assert_eq!(redact(raw, None, &[]), raw);
        }
    }

    #[test]
    fn a_locked_entry_without_a_folder_does_not_double_the_noun() {
        let r = |lang, values: &Values| render("error.locked", values, lang, None, &[], &mut quiet()).text;
        let none = Values::default();
        assert_eq!(
            r(Lang::En, &none).matches("folder").count(),
            1,
            "{}",
            r(Lang::En, &none)
        );
        assert_eq!(
            r(Lang::Ar, &none).matches("المجلد").count(),
            1,
            "{}",
            r(Lang::Ar, &none)
        );
        let named = Values::default().folder("Photos");
        assert!(r(Lang::En, &named).contains("Photos"));
        assert!(r(Lang::Ar, &named).contains("Photos"));
        assert_eq!(
            r(Lang::En, &named).matches("folder").count(),
            0,
            "{}",
            r(Lang::En, &named)
        );
    }

    #[test]
    fn every_folder_root_the_caller_knows_is_shortened() {
        let d = redact_with(
            "a /home/me/Photos/docs/x.txt, b /home/me/Work/z.txt, c /home/me/PhotosOld/q.txt",
            &["/home/me/Work", "/home/me/Photos"],
            &[],
        );
        assert_eq!(d, "a Photos/…/x.txt, b Work/z.txt, c …");
        // One root through the older entry point is the same.
        assert_eq!(
            redact("/home/me/Work/z.txt", Some("/home/me/Work"), &[]),
            "Work/z.txt"
        );
    }

    #[test]
    fn a_missing_count_reads_as_a_real_zero() {
        let r = |lang| {
            render(
                "status.folders_need_you",
                &Values::default(),
                lang,
                None,
                &[],
                &mut quiet(),
            )
            .text
        };
        assert_eq!(r(Lang::En), "0 folders need you");
        // Arabic has its own form for none, not "0" under the form for many.
        assert_eq!(r(Lang::Ar), "لا مجلدات تحتاج إليك");
    }

    #[test]
    fn a_partial_run_with_no_count_claims_no_number() {
        let ent = entry("run.partial_some").expect("the entry without a number");
        assert_eq!(ent.slots, &[Slot::Folder]);
        for t in ent.en.all().chain(ent.ar.all()) {
            assert!(!t.contains("{count}"), "{t}");
            assert!(!t.chars().any(|c| c.is_ascii_digit()), "{t}");
        }
        let r = render(
            "run.partial_some",
            &Values::default().folder("Docs"),
            Lang::En,
            Some("boom"),
            &[],
            &mut quiet(),
        );
        assert_eq!(
            r.text,
            "Wasl couldn't sync everything in Docs. It will try again at the next sync."
        );
        assert_eq!(r.fix, Some(Fix::Retry));
    }

    #[test]
    fn a_catch_all_failure_promises_only_what_the_engine_does() {
        // The engine retries a failed run at the next sync. Nothing here says it retries "by itself",
        // right away or in a moment for an error whose cause isn't known.
        for key in ["error.io", "error.network"] {
            let ent = entry(key).unwrap();
            for t in ent.en.all() {
                assert!(t.contains("next sync"), "{key}: {t}");
                assert!(!t.contains("by itself") && !t.contains("moment"), "{key}: {t}");
            }
            for t in ent.ar.all() {
                assert!(t.contains("المزامنة التالية"), "{key}: {t}");
                assert!(!t.contains("من تلقاء نفسه"), "{key}: {t}");
            }
        }
    }

    #[test]
    fn a_disk_quota_is_not_a_full_device() {
        let q = entry("error.disk_quota").expect("an entry for a quota on this device");
        assert_eq!(q.slots, &[File, Folder]);
        for t in q.en.all() {
            assert!(t.contains("limit"), "{t}");
            assert!(!t.contains("is full"), "{t}");
        }
        for t in q.ar.all() {
            assert!(!t.contains("ممتلئة"), "{t}");
        }
        // The disk itself being full keeps its own words.
        assert!(entry("error.disk_full").unwrap().en.other.contains("is full"));
    }

    #[test]
    fn a_locked_entry_uses_the_apps_word_not_vault() {
        for key in ["error.locked", "request.locked"] {
            let ent = entry(key).unwrap();
            for t in ent.en.all() {
                assert!(!t.to_lowercase().contains("vault"), "{key}: {t}");
            }
            for t in ent.ar.all() {
                assert!(!t.contains("الخزنة"), "{key}: {t}");
            }
        }
        assert!(
            entry("error.locked")
                .unwrap()
                .en
                .other
                .contains("encrypted and locked")
        );
    }

    #[test]
    fn reference_doc_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/STATUS_WORDS.md");
        let want = reference_markdown();
        if std::env::var_os("WASL_BLESS").is_some() {
            std::fs::write(&path, &want).unwrap();
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            have.replace("\r\n", "\n"),
            want,
            "docs/STATUS_WORDS.md is stale: WASL_BLESS=1 cargo test -p wasl-proto words"
        );
    }
}
