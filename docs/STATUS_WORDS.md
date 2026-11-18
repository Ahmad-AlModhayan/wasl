# Status and error words

Generated from `engine/crates/wasl-proto/src/words.rs`; do not edit by hand. Run
`WASL_BLESS=1 cargo test -p wasl-proto words` after changing the catalogue.

Every status and error Wasl shows is one of these entries. The key is stable: a script or an
agent acts on the key, the typed values and the fix, never on the sentence. Values in the words
are written `{name}`; the engine fills them and renders the entry in the caller's language,
falling back to English.

## `overall.synced`

- Note: Short status of the whole app when every folder is up to date.
- Values: none
- Fix: none
- English: Synced
- Arabic: تمت المزامنة

## `overall.syncing`

- Note: Short status of the whole app while any folder is syncing.
- Values: none
- Fix: none
- English: Syncing
- Arabic: تجري المزامنة

## `overall.paused`

- Note: Short status of the whole app when syncing is paused.
- Values: none
- Fix: none
- English: Paused
- Arabic: متوقف مؤقتًا

## `overall.waiting`

- Note: Short status of the whole app when folders wait (no network, a schedule, a first preview).
- Values: none
- Fix: none
- English: Waiting
- Arabic: في الانتظار

## `overall.needs_you`

- Note: Short status of the whole app when something needs the person.
- Values: none
- Fix: none
- English: Needs you
- Arabic: يحتاج إليك

## `overall.idle`

- Note: Short status of the whole app before any folder is added.
- Values: none
- Fix: none
- English: No folders yet
- Arabic: لا توجد مجلدات بعد

## `status.folders_need_you`

- Note: Badge or heading on the home screen. {count} is the number of folders that need the person.
- Values: count
- Fix: none
- English (one): {count} folder needs you
- English (other): {count} folders need you
- Arabic (zero): لا مجلدات تحتاج إليك
- Arabic (one): مجلد واحد يحتاج إليك
- Arabic (two): مجلدان يحتاجان إليك
- Arabic (few): {count} مجلدات تحتاج إليك
- Arabic (many): {count} مجلدًا يحتاج إليك
- Arabic (other): {count} مجلد يحتاج إليك

## `error.auth`

- Note: Shown on a folder or in a notification when Google no longer accepts the saved sign-in.
- Values: folder
- Fix: `sign_in_again`
- English: Your Google sign-in has ended, so {folder} can't sync. Nothing was changed.
- Arabic: انتهى تسجيل دخولك إلى Google، لذلك لا يمكن مزامنة {folder}. لم يتغير شيء.

## `error.permission`

- Note: The folder or the account refused Wasl access; the run stops (a fatal error). {folder} is the synced folder.
- Values: folder
- Fix: `open_folder`
- English: Wasl isn't allowed to use {folder} any more, so it stopped syncing there. Nothing was changed.
- Arabic: لم يعد وصل مسموحًا له باستخدام {folder}، لذلك توقفت المزامنة هناك. لم يتغير شيء.

## `error.quota`

- Note: The person's Drive storage is full (not this device's disk).
- Values: folder
- Fix: `open_drive_storage`
- English: Google Drive is full, so {folder} can't send more files. Nothing was lost.
- Arabic: مساحة Google Drive ممتلئة، لذلك لا يستطيع {folder} رفع المزيد من الملفات. لم يضع شيء.

## `error.not_found`

- Note: Something the run needed (a Drive folder, an upload) is gone. Not one vanished file: that is error.file_gone.
- Values: folder
- Fix: `retry`
- English: Wasl couldn't find something it needed for {folder}. Nothing was changed.
- Arabic: تعذّر على وصل العثور على شيء يحتاجه لـ {folder}. لم يتغير شيء.

## `error.locked`

- Note: Another program holds the file open or locked.
- Values: file
- Fix: `retry`
- English: {file} is open in another program, so Wasl left it for now. It will be tried again at the next sync.
- Arabic: {file} مفتوح في برنامج آخر، لذلك تركه وصل الآن. ستتم محاولته مجددًا في المزامنة التالية.

## `error.network`

- Note: Shown while the network is down or flaky. A passing failure; only becomes 'needs you' when retries give up.
- Values: none
- Fix: `retry`
- English: Wasl can't reach Google Drive right now. It will try again by itself.
- Arabic: لا يستطيع وصل الوصول إلى Google Drive الآن. سيحاول مجددًا من تلقاء نفسه.

## `error.rate_limit`

- Note: Google answered with 'too many requests'.
- Values: none
- Fix: `retry`
- English: Google Drive asked Wasl to slow down. It will try again in a moment.
- Arabic: طلب Google Drive من وصل أن يبطئ. سيحاول مجددًا بعد قليل.

## `error.server`

- Note: Google answered with a server error.
- Values: none
- Fix: `retry`
- English: Google Drive had a problem on its side. Wasl will try again at the next sync.
- Arabic: واجه Google Drive مشكلة من جانبه. سيحاول وصل مجددًا في المزامنة التالية.

## `error.io`

- Note: A local file operation failed for a reason Wasl can't name more exactly.
- Values: folder
- Fix: `open_folder`
- English: Wasl couldn't read or write a file in {folder}. It will be tried again at the next sync.
- Arabic: تعذّر على وصل قراءة ملف في {folder} أو الكتابة فيه. ستتم محاولته مجددًا في المزامنة التالية.

## `error.integrity`

- Note: A downloaded or received file failed its check.
- Values: folder
- Fix: `retry`
- English: A file in {folder} didn't arrive intact, so Wasl didn't keep it. It will be tried again.
- Arabic: لم يصل ملف في {folder} سليمًا، لذلك لم يحتفظ به وصل. ستتم محاولته مجددًا.

## `request.not_found`

- Note: The engine has no folder, device, file or vault by that name. Details carries the engine's own message.
- Values: none
- Fix: none
- English: Wasl couldn't find what you asked for.
- Arabic: تعذّر على وصل العثور على ما طلبته.

## `request.permission`

- Note: The engine refused a request the person isn't allowed to make. Details carries the engine's own message.
- Values: none
- Fix: none
- English: Wasl isn't allowed to do that.
- Arabic: لا يملك وصل الإذن لتنفيذ ذلك.

## `request.locked`

- Note: The engine can't do a request because something is held open or locked. Details carries the engine's message.
- Values: none
- Fix: `retry`
- English: That is in use right now, so Wasl can't do it yet.
- Arabic: هذا قيد الاستخدام الآن، لذلك لا يستطيع وصل تنفيذه بعد.

## `error.file_gone`

- Note: A file was deleted or moved between Wasl listing it and reading it. Not a problem; the next sync is clean.
- Values: file, folder
- Fix: `retry`
- English: {file} was gone before Wasl could sync it. Skipped it; the rest of {folder} synced.
- Arabic: اختفى {file} قبل أن يتمكن وصل من مزامنته. تم تخطيه ومزامنة بقية {folder}.

## `error.disk_full`

- Note: The disk or phone storage on THIS device is full (contrast error.quota: the person's Drive is full).
- Values: file, folder
- Fix: `open_storage_settings`
- English: This device's storage is full, so Wasl couldn't save {file} in {folder}. Nothing was lost.
- Arabic: مساحة التخزين في هذا الجهاز ممتلئة، لذلك تعذّر على وصل حفظ {file} في {folder}. لم يضع شيء.

## `error.name_refused`

- Note: The file name is too long, has a character or a reserved name that one side refuses. Wasl never renames it.
- Values: file, folder
- Fix: `open_to_rename`
- English: Drive or this device won't accept the name of {file}. Skipped it; the rest of {folder} synced.
- Arabic: لا يقبل Drive أو هذا الجهاز اسم {file}. تم تخطيه ومزامنة بقية {folder}.

## `error.invalid`

- Note: Shown for a request the engine refused; Details carries the engine's own message.
- Values: none
- Fix: none
- English: Wasl couldn't do that: the request wasn't valid.
- Arabic: تعذّر على وصل تنفيذ ذلك: الطلب غير صالح.

## `error.busy`

- Note: The folder is in the middle of a sync or another change.
- Values: none
- Fix: `retry`
- English: Wasl is busy with that folder right now. Try again in a moment.
- Arabic: وصل مشغول بهذا المجلد الآن. حاول مجددًا بعد قليل.

## `error.unsupported`

- Note: The engine doesn't support the request (usually an older engine).
- Values: none
- Fix: `update_wasl`
- English: This version of Wasl can't do that yet.
- Arabic: هذا الإصدار من وصل لا يستطيع تنفيذ ذلك بعد.

## `error.unknown`

- Note: Fallback for any error the catalogue doesn't know. Always has Details; each unknown error is logged.
- Values: folder
- Fix: `retry`
- English: Something went wrong syncing {folder}. Wasl will try again at the next sync.
- Arabic: حدث خطأ أثناء مزامنة {folder}. سيحاول وصل مجددًا في المزامنة التالية.

## `daemon.not_running`

- Note: The command line couldn't find the background program. The command stays in English.
- Values: none
- Fix: none
- English: Wasl isn't running. Start it with: wasl daemon start
- Arabic: وصل لا يعمل. شغّله بالأمر: wasl daemon start

## `daemon.untrusted`

- Note: Whatever answers on Wasl's local address didn't prove it is this user's Wasl.
- Values: none
- Fix: none
- English: The program answering is not this user's Wasl.
- Arabic: البرنامج الذي يجيب ليس وصل الخاص بهذا المستخدم.

## `daemon.version_mismatch`

- Note: The background program and the app speak different protocol versions. Details has both numbers.
- Values: none
- Fix: `update_wasl`
- English: The Wasl running in the background and this app don't speak the same version. Update Wasl.
- Arabic: وصل العامل في الخلفية وهذا التطبيق لا يستخدمان الإصدار نفسه. حدّث وصل.

## `daemon.connection_lost`

- Note: The link to the background program broke in the middle of a request.
- Values: none
- Fix: `retry`
- English: Lost the connection to Wasl. Try again.
- Arabic: انقطع الاتصال بوصل. حاول مجددًا.

