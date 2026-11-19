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

## `overall.partly_synced`

- Note: Short status of the whole app when a folder's last run moved some files and not others. Not 'synced', not 'failed'.
- Values: none
- Fix: none
- English: Partly synced
- Arabic: تمت المزامنة جزئيًا

## `status.items_need_you`

- Note: Badge or heading on the home screen. {count} is the number of needs-you items across all folders (one folder can hold several).
- Values: count
- Fix: none
- English (one): {count} thing needs you
- English (other): {count} things need you
- Arabic (zero): لا شيء يحتاج إليك
- Arabic (one): شيء واحد يحتاج إليك
- Arabic (two): شيئان يحتاجان إليك
- Arabic (few): {count} أشياء تحتاج إليك
- Arabic (many): {count} شيئًا يحتاج إليك
- Arabic (other): {count} شيء يحتاج إليك

## `folder.synced`

- Note: A folder whose last run moved everything and which has nothing waiting.
- Values: folder
- Fix: none
- English: {folder} is up to date.
- Arabic: {folder} محدَّث.

## `folder.syncing`

- Note: A folder is syncing now.
- Values: folder
- Fix: none
- English: Syncing {folder}…
- Arabic: تجري مزامنة {folder}…

## `folder.off`

- Note: The person turned the folder off; nothing syncs.
- Values: folder
- Fix: none
- English: {folder} is turned off.
- Arabic: {folder} غير مفعّل.

## `folder.paused`

- Note: The person paused the folder; it syncs again when resumed.
- Values: folder
- Fix: none
- English: {folder} is paused.
- Arabic: {folder} متوقف مؤقتًا.

## `folder.waiting_no_network`

- Note: The device is offline, or the last run couldn't reach Google Drive. Nothing is lost; the next sync carries on.
- Values: folder
- Fix: `retry`
- English: {folder} will sync when Wasl is back online.
- Arabic: ستتم مزامنة {folder} عندما يعود وصل إلى الاتصال بالإنترنت.

## `folder.waiting_schedule`

- Note: The folder's own conditions hold the next run (Wi-Fi, charging, battery, VPN), or its first sync hasn't come round yet.
- Values: folder
- Fix: none
- English: {folder} is waiting for the right time to sync.
- Arabic: {folder} ينتظر الوقت المناسب للمزامنة.

## `folder.waiting_device`

- Note: A folder that syncs only between devices waits for a device that is offline now. {device} is that device's name.
- Values: folder, device
- Fix: none
- English: {folder} will sync when {device} is back online.
- Arabic: ستتم مزامنة {folder} عندما يعود {device} إلى الاتصال.

## `folder.cant_reach_device`

- Note: A folder that syncs only between devices has a device Wasl hasn't reached for a long time (or never). {device} is that device's name.
- Values: folder, device
- Fix: none
- English: Wasl can't reach {device}, so {folder} isn't synced there.
- Arabic: لا يستطيع وصل الوصول إلى {device}، لذلك لم تتم مزامنة {folder} هناك.

## `device.online`

- Note: Status of a device in the list of the person's devices: connected now and not syncing.
- Values: device
- Fix: none
- English: Online
- Arabic: متصل

## `device.syncing`

- Note: Status of a device in the list of the person's devices: connected now and a folder is syncing with it.
- Values: device
- Fix: none
- English: Syncing
- Arabic: تجري المزامنة

## `device.last_seen_now`

- Note: Status of a device that is offline and was connected less than a minute ago.
- Values: device
- Fix: none
- English: Last seen a moment ago
- Arabic: آخر ظهور قبل لحظات

## `device.last_seen_minutes`

- Note: Status of a device that is offline: when it was last connected, in minutes (1 to 59). {count} is the number of minutes.
- Values: device, count
- Fix: none
- English (one): Last seen {count} minute ago
- English (other): Last seen {count} minutes ago
- Arabic (zero): آخر ظهور قبل أقل من دقيقة
- Arabic (one): آخر ظهور قبل دقيقة واحدة
- Arabic (two): آخر ظهور قبل دقيقتين
- Arabic (few): آخر ظهور قبل {count} دقائق
- Arabic (many): آخر ظهور قبل {count} دقيقة
- Arabic (other): آخر ظهور قبل {count} دقيقة

## `device.last_seen_hours`

- Note: Status of a device that is offline: when it was last connected, in hours (1 to 23). {count} is the number of hours.
- Values: device, count
- Fix: none
- English (one): Last seen {count} hour ago
- English (other): Last seen {count} hours ago
- Arabic (zero): آخر ظهور قبل أقل من ساعة
- Arabic (one): آخر ظهور قبل ساعة واحدة
- Arabic (two): آخر ظهور قبل ساعتين
- Arabic (few): آخر ظهور قبل {count} ساعات
- Arabic (many): آخر ظهور قبل {count} ساعة
- Arabic (other): آخر ظهور قبل {count} ساعة

## `device.last_seen_days`

- Note: Status of a device that is offline: when it was last connected, in days (1 to 30). {count} is the number of days.
- Values: device, count
- Fix: none
- English (one): Last seen {count} day ago
- English (other): Last seen {count} days ago
- Arabic (zero): آخر ظهور قبل أقل من يوم
- Arabic (one): آخر ظهور قبل يوم واحد
- Arabic (two): آخر ظهور قبل يومين
- Arabic (few): آخر ظهور قبل {count} أيام
- Arabic (many): آخر ظهور قبل {count} يومًا
- Arabic (other): آخر ظهور قبل {count} يوم

## `device.cant_reach`

- Note: Status of a device that was never reached from here, or not for more than 30 days.
- Values: device
- Fix: none
- English: Can't reach
- Arabic: تعذّر الوصول إليه

## `pair.bad_code`

- Note: The person typed something that isn't an 8-character pairing code (or a scanned code). Case, spaces and dashes don't matter. Shown beside the code field.
- Values: none
- Fix: none
- English: That isn't a pairing code. It is 8 letters and numbers, like K7QM-2XNP.
- Arabic: هذا ليس رمز اقتران. هو 8 أحرف وأرقام، مثل K7QM-2XNP.

## `pair.wrong_code`

- Note: The other device did not prove it knows the code: a typo. A mistyped code is used up, so the person makes a new one.
- Values: none
- Fix: none
- English: That code didn't match. Check the 8 characters, or make a new code on the other device and try again.
- Arabic: لم يتطابق هذا الرمز. تحقق من الأحرف الثمانية، أو أنشئ رمزًا جديدًا على الجهاز الآخر وحاول مجددًا.

## `pair.code_expired`

- Note: A pairing code works for 10 minutes. Shown on either device.
- Values: none
- Fix: none
- English: That code has expired. Make a new code on the other device and try again.
- Arabic: انتهت صلاحية هذا الرمز. أنشئ رمزًا جديدًا على الجهاز الآخر وحاول مجددًا.

## `pair.code_used`

- Note: A pairing code works once, even when the first try failed.
- Values: none
- Fix: none
- English: That code was already used. Make a new code on the other device and try again.
- Arabic: استُخدم هذا الرمز من قبل. أنشئ رمزًا جديدًا على الجهاز الآخر وحاول مجددًا.

## `pair.not_found`

- Note: The device typing the code found nobody open for pairing, or could not reach who it found.
- Values: none
- Fix: none
- English: Couldn't find a device showing that code. Check that both devices are on the same network and the code is still on its screen.
- Arabic: تعذّر العثور على جهاز يعرض هذا الرمز. تأكد أن الجهازين على الشبكة نفسها وأن الرمز لا يزال ظاهرًا على شاشته.

## `pair.device_removed`

- Note: A removed device tried to pair with its old identity. Nothing was used up on the other device. It starts clean on restart.
- Values: none
- Fix: none
- English: This device was removed from your group. Restart Wasl on it, then pair it again as a new device.
- Arabic: أُزيل هذا الجهاز من مجموعتك. أعد تشغيل وصل عليه، ثم أضِفه من جديد كجهاز جديد.

## `pair.cancelled`

- Note: The person said the words don't match, cancelled, or the other device stopped pairing.
- Values: none
- Fix: none
- English: Pairing was stopped on one of the devices. A mistyped code stops it too. Make a new code to try again.
- Arabic: أُوقف الاقتران على أحد الجهازين. وإدخال رمز خاطئ يوقفه أيضًا. أنشئ رمزًا جديدًا للمحاولة مجددًا.

## `pair.timed_out`

- Note: A device answered a typed code but never got as far as showing the words, or the pairing ran out of time.
- Values: none
- Fix: none
- English: No device answered in time. Check that both are on the same network and the code is still showing, then try again.
- Arabic: لم يردّ أي جهاز في الوقت المناسب. تأكد أن الجهازين على الشبكة نفسها وأن الرمز ما زال ظاهرًا، ثم حاول مرة أخرى.

## `pair.own_code`

- Note: The person scanned or typed a QR code that this very device made.
- Values: none
- Fix: none
- English: That code was made on this device. Type the code shown on the other device.
- Arabic: هذا الرمز أُنشئ على هذا الجهاز. اكتب الرمز الظاهر على الجهاز الآخر.

## `pair.other`

- Note: Catch-all for a pairing that ended for a reason the catalogue doesn't name (the other device went quiet, the network broke).
- Values: none
- Fix: none
- English: Pairing didn't work. Make a new code and try again.
- Arabic: لم ينجح الاقتران. أنشئ رمزًا جديدًا وحاول مجددًا.

## `folder.needs_you`

- Note: Heading of a folder that has needs-you items; each item has its own entry.
- Values: folder
- Fix: none
- English: {folder} needs you.
- Arabic: {folder} يحتاج إليك.

## `folder.first_preview`

- Note: A new folder waits for the person to start it from the first preview. Nothing moves until then.
- Values: folder
- Fix: `review`
- English: Look over what Wasl will do in {folder}, then start syncing.
- Arabic: راجع ما سيفعله وصل في {folder} ثم ابدأ المزامنة.

## `folder.needs_approval`

- Note: A safety guard (many deletions, for example) stopped the run and waits for the person's yes.
- Values: folder
- Fix: `review`
- English: Wasl stopped {folder} to check with you before a large change.
- Arabic: أوقف وصل {folder} ليسألك قبل إجراء تغيير كبير.

## `folder.conflicts`

- Note: Open conflicts wait for the person. {count} is the number of files.
- Values: folder, count
- Fix: `review`
- English (one): {count} file in {folder} was changed in two places and needs your choice.
- English (other): {count} files in {folder} were changed in two places and need your choice.
- Arabic (zero): لا ملفات في {folder} تحتاج إلى اختيارك.
- Arabic (one): ملف واحد في {folder} تغيّر في مكانين ويحتاج إلى اختيارك.
- Arabic (two): ملفان في {folder} تغيّرا في مكانين ويحتاجان إلى اختيارك.
- Arabic (few): {count} ملفات في {folder} تغيّرت في مكانين وتحتاج إلى اختيارك.
- Arabic (many): {count} ملفًا في {folder} تغيّرت في مكانين وتحتاج إلى اختيارك.
- Arabic (other): {count} ملف في {folder} تغيّرت في مكانين وتحتاج إلى اختيارك.

## `run.partial`

- Note: Shown after a run that finished with some items failing. {count} is how many items failed; Details has the first cause.
- Values: folder, count
- Fix: `retry`
- English (one): {count} item in {folder} didn't sync. Wasl will try again at the next sync.
- English (other): {count} items in {folder} didn't sync. Wasl will try again at the next sync.
- Arabic (zero): لا عناصر في {folder} لم تتم مزامنتها. سيحاول وصل مجددًا في المزامنة التالية.
- Arabic (one): عنصر واحد في {folder} لم تتم مزامنته. سيحاول وصل مجددًا في المزامنة التالية.
- Arabic (two): عنصران في {folder} لم تتم مزامنتهما. سيحاول وصل مجددًا في المزامنة التالية.
- Arabic (few): {count} عناصر في {folder} لم تتم مزامنتها. سيحاول وصل مجددًا في المزامنة التالية.
- Arabic (many): {count} عنصرًا في {folder} لم تتم مزامنتها. سيحاول وصل مجددًا في المزامنة التالية.
- Arabic (other): {count} عنصر في {folder} لم تتم مزامنتها. سيحاول وصل مجددًا في المزامنة التالية.

## `run.partial_some`

- Note: Shown after a run that finished with some items failing when the message doesn't say how many (one failing item, or an older engine). Details has the first cause.
- Values: folder
- Fix: `retry`
- English: Wasl couldn't sync everything in {folder}. It will try again at the next sync.
- Arabic: تعذّر على وصل مزامنة كل شيء في {folder}. سيحاول مجددًا في المزامنة التالية.

## `error.auth`

- Note: Shown on a folder or in a notification when Google no longer accepts the saved sign-in.
- Values: folder
- Fix: `sign_in_again`
- English: Your Google sign-in has ended, so {folder} can't sync.
- Arabic: انتهى تسجيل دخولك إلى Google، لذلك لا يمكن مزامنة {folder}.

## `error.permission`

- Note: The folder or the account refused Wasl access; the run stops (a fatal error). {folder} is the synced folder.
- Values: folder
- Fix: `open_folder`
- English: Wasl isn't allowed to use {folder} any more, so it stopped syncing there.
- Arabic: لم يعد وصل مسموحًا له باستخدام {folder}، لذلك توقفت المزامنة هناك.

## `error.quota`

- Note: The person's Drive storage is full (not this device's disk).
- Values: folder
- Fix: `open_drive_storage`
- English: Google Drive is full, so {folder} can't send more files.
- Arabic: مساحة Google Drive ممتلئة، لذلك لا يستطيع {folder} رفع المزيد من الملفات.

## `error.not_found`

- Note: Something the run needed (a Drive folder, an upload) is gone. Not one vanished file: that is error.file_gone.
- Values: folder
- Fix: `retry`
- English: Wasl couldn't find something it needed for {folder}.
- Arabic: تعذّر على وصل العثور على شيء يحتاجه لـ {folder}.

## `error.locked`

- Note: A vault folder whose keys this device doesn't hold (locked, or its keys unusable). Not a file held open by another program.
- Values: folder
- Fix: `unlock_vault`
- English: Wasl can't sync {folder}: it is encrypted and locked on this device. Unlock it, then try again.
- Arabic: لا يستطيع وصل مزامنة {folder}: فهو مشفّر ومقفل على هذا الجهاز. افتح قفله ثم حاول مجددًا.

## `error.network`

- Note: A run ended because Google Drive couldn't be reached. The only promise made is the next sync.
- Values: none
- Fix: `retry`
- English: Wasl can't reach Google Drive right now. It will try again at the next sync.
- Arabic: لا يستطيع وصل الوصول إلى Google Drive الآن. سيحاول مجددًا في المزامنة التالية.

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
- English: Wasl couldn't read or write a file in {folder}. It will try again at the next sync.
- Arabic: تعذّر على وصل قراءة ملف في {folder} أو الكتابة فيه. سيحاول مجددًا في المزامنة التالية.

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

## `request.auth`

- Note: A request needed a Google sign-in the engine doesn't have. Details carries the engine's own message.
- Values: none
- Fix: `sign_in_again`
- English: Wasl isn't signed in to that Google account, or the sign-in has ended.
- Arabic: وصل غير مسجّل الدخول إلى حساب Google هذا، أو انتهى تسجيل الدخول.

## `request.network`

- Note: A request needed Google Drive and the network failed. Details carries the engine's own message.
- Values: none
- Fix: `retry`
- English: Wasl couldn't reach Google Drive for that. Check the connection and try again.
- Arabic: تعذّر على وصل الوصول إلى Google Drive لتنفيذ ذلك. تحقق من الاتصال وحاول مجددًا.

## `request.other`

- Note: Catch-all for a request failure the catalogue can't name; each one is logged. Details carries the engine's own message.
- Values: none
- Fix: `retry`
- English: Something went wrong with that request.
- Arabic: حدث خطأ أثناء تنفيذ هذا الطلب.

## `request.permission`

- Note: The engine refused a request the person isn't allowed to make. Details carries the engine's own message.
- Values: none
- Fix: none
- English: Wasl isn't allowed to do that.
- Arabic: لا يملك وصل الإذن لتنفيذ ذلك.

## `request.locked`

- Note: A vault, or a view of one, is locked on this device so the engine can't do the request. Details carries the engine's message.
- Values: none
- Fix: `unlock_vault`
- English: That is locked on this device. Unlock it, then try again.
- Arabic: هذا مقفل على هذا الجهاز. افتح قفله ثم حاول مجددًا.

## `error.file_gone`

- Note: A file was deleted or moved between Wasl listing it and reading it. Not a problem; the next sync is clean. The engine records it as a FILE_GONE run event ([`event_key`]); {file} is the file's name, not its path.
- Values: file
- Fix: `retry`
- English: {file} was gone before Wasl could sync it, so it was skipped.
- Arabic: اختفى {file} قبل أن يتمكن وصل من مزامنته، لذلك تم تخطيه.

## `error.disk_full`

- Note: The disk or phone storage on THIS device is full (contrast error.quota: the person's Drive is full).
- Values: file, folder
- Fix: `open_storage_settings`
- English: This device's storage is full, so Wasl couldn't save {file} in {folder}.
- Arabic: مساحة التخزين في هذا الجهاز ممتلئة، لذلك تعذّر على وصل حفظ {file} في {folder}.

## `error.disk_quota`

- Note: A per-user disk quota on THIS device is used up (EDQUOT), though the disk itself may have room. Not error.disk_full (the device is full) and not error.quota (the person's Drive is full).
- Values: file, folder
- Fix: none
- English: The storage limit set for you on this device is reached, so Wasl couldn't save {file} in {folder}. Free some space, or ask whoever set the limit.
- Arabic: تم بلوغ حدّ التخزين المحدد لك على هذا الجهاز، لذلك تعذّر على وصل حفظ {file} في {folder}. حرّر بعض المساحة أو اطلب من الجهة التي حدّدت الحدّ رفعه.

## `error.name_refused`

- Note: The file name is too long, has a character or a reserved name that one side refuses. Wasl never renames it.
- Values: file
- Fix: `open_to_rename`
- English: Drive or this device won't accept the name of {file}, so it was skipped.
- Arabic: لا يقبل Drive أو هذا الجهاز اسم {file}، لذلك تم تخطيه.

## `error.folder_missing`

- Note: The synced folder itself is gone from THIS device (a card taken out, the folder deleted or moved), so the run stopped. Not one vanished file (error.file_gone) and not a refused permission (error.permission). Nothing was deleted on Drive.
- Values: folder
- Fix: `open_folder`
- English: {folder} can't be found on this device. If it is on a memory card or an external drive, reconnect it; otherwise choose the folder again.
- Arabic: تعذّر العثور على {folder} في هذا الجهاز. إن كان على بطاقة ذاكرة أو قرص خارجي فأعد توصيله، وإلا فاختر المجلد من جديد.

## `error.sign_in_unreadable`

- Note: No usable sign-in is saved on this device: it was never saved here, or the system's keychain no longer gives it up. Contrast error.auth: Google no longer accepts a sign-in Wasl can read.
- Values: folder
- Fix: `sign_in_again`
- English: Wasl has no usable Google sign-in saved on this device, so {folder} can't sync. Sign in again.
- Arabic: ليس لدى وصل تسجيل دخول صالح إلى Google محفوظ على هذا الجهاز، لذلك لا يمكن مزامنة {folder}. سجّل الدخول مجددًا.

## `account.sign_in_ended`

- Note: A row of the account list, or a problem listed without a folder: the account's session ended on Google's side. The account's email is shown beside these words, not inside them.
- Values: none
- Fix: `sign_in_again`
- English: Google no longer accepts this account's sign-in. Sign in again.
- Arabic: لم تعد Google تقبل تسجيل الدخول لهذا الحساب. سجّل الدخول مجددًا.

## `account.sign_in_unreadable`

- Note: A row of the account list, or a problem listed without a folder: no usable sign-in is saved on this device (never saved here, or the keychain no longer gives it up). The account's email is shown beside these words, not inside them.
- Values: none
- Fix: `sign_in_again`
- English: Wasl has no usable sign-in saved for this account on this device. Sign in again.
- Arabic: ليس لدى وصل تسجيل دخول صالح محفوظ لهذا الحساب على هذا الجهاز. سجّل الدخول مجددًا.

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

## `daemon.client_needs_update`

- Note: The command line or app speaks only older protocol versions than the background program. Details says which side is behind.
- Values: none
- Fix: `update_wasl`
- English: This Wasl is older than the one running in the background, so they can't talk to each other. Update Wasl.
- Arabic: هذا الإصدار من وصل أقدم من الذي يعمل في الخلفية، لذلك لا يستطيعان التخاطب. حدّث وصل.

## `daemon.daemon_needs_update`

- Note: The background program speaks only older protocol versions than the command line or app. Details says which side is behind.
- Values: none
- Fix: `update_wasl`
- English: The Wasl running in the background is older than this one, so they can't talk to each other. Update Wasl and start it again.
- Arabic: وصل العامل في الخلفية أقدم من هذا الإصدار، لذلك لا يستطيعان التخاطب. حدّث وصل وأعد تشغيله.

## `daemon.connection_lost`

- Note: The link to the background program broke in the middle of a request.
- Values: none
- Fix: `retry`
- English: Lost the connection to Wasl. Try again.
- Arabic: انقطع الاتصال بوصل. حاول مجددًا.

