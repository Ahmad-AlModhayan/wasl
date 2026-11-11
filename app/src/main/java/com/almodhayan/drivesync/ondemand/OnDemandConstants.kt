package com.almodhayan.drivesync.ondemand

import android.content.Intent
import android.provider.DocumentsContract

/** Shared constants so the UI can link to the on-demand Drive provider without depending on its implementation. */
object OnDemandConstants {
    const val AUTHORITY = "com.kahflabs.wasl.documents"

    /** Root id convention: one root per account, identified by the account email. */
    fun rootId(accountId: String): String = accountId

    /** Document id convention: "<accountEmail>:<driveFileId>". */
    fun documentId(accountId: String, fileId: String): String = "$accountId:$fileId"

    /** Intent that opens the system Files app at this account's Drive root. */
    fun openFilesIntent(accountId: String): Intent =
        Intent(Intent.ACTION_VIEW).setDataAndType(DocumentsContract.buildRootUri(AUTHORITY, rootId(accountId)), DocumentsContract.Root.MIME_TYPE_ITEM)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
}
