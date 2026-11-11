package com.almodhayan.drivesync.core

/** Coarse error categories persisted on a pair, so the UI can name the one fix without the exception. */
enum class ErrorKind {
    AUTH, PERMISSION, QUOTA, NOT_FOUND, LOCKED, NETWORK, RATE_LIMIT, SERVER, IO, INTEGRITY, OTHER;

    /** Retrying on its own: never badged until it keeps failing. */
    val transient: Boolean get() = this == NETWORK || this == RATE_LIMIT || this == SERVER

    companion object {
        fun of(e: AppError): ErrorKind = when (e) {
            is AppError.Auth -> AUTH
            is AppError.Permission -> PERMISSION
            is AppError.StorageQuota -> QUOTA
            is AppError.NotFound -> NOT_FOUND
            is AppError.Locked -> LOCKED
            is AppError.Network -> NETWORK
            is AppError.RateLimited -> RATE_LIMIT
            is AppError.Http -> if (e.code >= 500) SERVER else OTHER
            is AppError.Io -> IO
            is AppError.Integrity -> INTEGRITY
            else -> OTHER
        }

        fun parse(s: String?): ErrorKind? = s?.let { runCatching { valueOf(it) }.getOrNull() }

        /**
         * The kind to show for a pair's failed run. "Android removed access, grant access again" is
         * only true for a folder chosen in the system picker once the app no longer holds its grant
         * ([granted], asked only then). With the grant still held, the failure came from something
         * else (Android refusing one file, #127): it shows as [IO], with the engine's own message
         * naming the file.
         */
        fun shown(kind: ErrorKind?, localPath: String, granted: (String) -> Boolean): ErrorKind? =
            if (kind == PERMISSION && localPath.startsWith("content://") && granted(localPath)) IO else kind
    }
}
