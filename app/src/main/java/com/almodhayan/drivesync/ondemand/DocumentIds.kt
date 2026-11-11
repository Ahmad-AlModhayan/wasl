package com.almodhayan.drivesync.ondemand

import java.io.FileNotFoundException

/** Parsed form of a SAF document id, see [OnDemandConstants.documentId] ("<accountEmail>:<driveFileId>"). */
data class DocId(val accountId: String, val fileId: String) {
    override fun toString(): String = OnDemandConstants.documentId(accountId, fileId)
}

/** Pure helpers for the document id convention. No Android types so it is unit-testable. */
object DocumentIds {
    /** Alias Drive accepts for the "My Drive" root before its real id is known (first crawl not done yet). */
    const val ROOT_ALIAS = "root"

    /**
     * Id of the virtual "Shared with me" folder shown under each root. Files shared by other people
     * have no parent in Drive, so without this folder they would be unreachable from the tree.
     * Real Drive ids are opaque base64url-like strings, so this literal cannot collide with one.
     */
    const val SHARED_WITH_ME = "shared-with-me"

    /**
     * Splits at the LAST ':'. Drive file ids never contain ':' (they are `[A-Za-z0-9_-]+`) while an
     * email local part may in theory, so the last separator is always the right one.
     */
    fun parse(documentId: String?): DocId? {
        if (documentId.isNullOrEmpty()) return null
        val i = documentId.lastIndexOf(':')
        if (i <= 0 || i == documentId.length - 1) return null
        return DocId(documentId.substring(0, i), documentId.substring(i + 1))
    }

    fun parseOrThrow(documentId: String?): DocId =
        parse(documentId) ?: throw FileNotFoundException("Not a Drive Sync document id: $documentId")

    fun isRootAlias(fileId: String): Boolean = fileId == ROOT_ALIAS
}
