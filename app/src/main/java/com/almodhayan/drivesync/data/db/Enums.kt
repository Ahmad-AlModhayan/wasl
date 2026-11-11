package com.almodhayan.drivesync.data.db

import androidx.annotation.StringRes
import com.almodhayan.drivesync.R
import com.almodhayan.drivesync.core.Strings

enum class SyncMode(@StringRes private val labelRes: Int, @StringRes private val descriptionRes: Int) {
    TWO_WAY(R.string.data_sync_mode_two_way, R.string.data_sync_mode_two_way_description),
    UPLOAD_ONLY(R.string.data_sync_mode_upload_only, R.string.data_sync_mode_upload_only_description),
    UPLOAD_THEN_DELETE(R.string.data_sync_mode_upload_then_delete, R.string.data_sync_mode_upload_then_delete_description),
    UPLOAD_MIRROR(R.string.data_sync_mode_upload_mirror, R.string.data_sync_mode_upload_mirror_description),
    DOWNLOAD_ONLY(R.string.data_sync_mode_download_only, R.string.data_sync_mode_download_only_description),
    DOWNLOAD_THEN_DELETE(R.string.data_sync_mode_download_then_delete, R.string.data_sync_mode_download_then_delete_description),
    DOWNLOAD_MIRROR(R.string.data_sync_mode_download_mirror, R.string.data_sync_mode_download_mirror_description);

    val label: String get() = Strings.get(labelRes)
    val description: String get() = Strings.get(descriptionRes)

    val uploads: Boolean get() = this == TWO_WAY || this == UPLOAD_ONLY || this == UPLOAD_THEN_DELETE || this == UPLOAD_MIRROR
    val downloads: Boolean get() = this == TWO_WAY || this == DOWNLOAD_ONLY || this == DOWNLOAD_THEN_DELETE || this == DOWNLOAD_MIRROR
}

enum class ConflictPolicy(@StringRes private val labelRes: Int) {
    KEEP_BOTH(R.string.data_conflict_policy_keep_both),
    NEWER_WINS(R.string.data_conflict_policy_newer_wins),
    LOCAL_WINS(R.string.data_conflict_policy_local_wins),
    REMOTE_WINS(R.string.data_conflict_policy_remote_wins),
    SKIP(R.string.data_conflict_policy_skip);

    val label: String get() = Strings.get(labelRes)
}

enum class RunStatus { RUNNING, SUCCESS, PARTIAL, FAILED, BLOCKED, SKIPPED, CANCELLED }

enum class EventLevel { INFO, WARN, ERROR }

enum class TrashReason { DELETED, OVERWRITTEN, CONFLICT, RESTORED_VERSION }

enum class DocsFormat(@StringRes private val labelRes: Int) {
    OFFICE(R.string.data_docs_format_office), PDF(R.string.data_docs_format_pdf);

    val label: String get() = Strings.get(labelRes)
}
