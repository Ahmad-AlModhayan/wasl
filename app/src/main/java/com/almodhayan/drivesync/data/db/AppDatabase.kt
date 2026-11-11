package com.almodhayan.drivesync.data.db

import android.content.Context
import androidx.room.AutoMigration
import androidx.room.Database
import androidx.room.Room
import androidx.room.RoomDatabase
import androidx.room.TypeConverter
import androidx.room.TypeConverters

class Converters {
    @TypeConverter fun modeToString(m: SyncMode?): String? = m?.name
    @TypeConverter fun stringToMode(s: String?): SyncMode? = s?.let { SyncMode.valueOf(it) }
    @TypeConverter fun policyToString(p: ConflictPolicy?): String? = p?.name
    @TypeConverter fun stringToPolicy(s: String?): ConflictPolicy? = s?.let { ConflictPolicy.valueOf(it) }
    @TypeConverter fun statusToString(s: RunStatus?): String? = s?.name
    @TypeConverter fun stringToStatus(s: String?): RunStatus? = s?.let { RunStatus.valueOf(it) }
    @TypeConverter fun levelToString(l: EventLevel?): String? = l?.name
    @TypeConverter fun stringToLevel(s: String?): EventLevel? = s?.let { EventLevel.valueOf(it) }
    @TypeConverter fun reasonToString(r: TrashReason?): String? = r?.name
    @TypeConverter fun stringToReason(s: String?): TrashReason? = s?.let { TrashReason.valueOf(it) }
    @TypeConverter fun docsToString(d: DocsFormat?): String? = d?.name
    @TypeConverter fun stringToDocs(s: String?): DocsFormat? = s?.let { DocsFormat.valueOf(it) }
}

@Database(
    entities = [
        AccountEntity::class, FolderPairEntity::class, FileStateEntity::class, SyncRunEntity::class,
        SyncEventEntity::class, ConflictEntity::class, TrashEntity::class, DriveNodeEntity::class,
        UploadSessionEntity::class, PinEntity::class, VaultEntity::class, BlockedPlanEntity::class,
    ],
    version = 3,
    exportSchema = true,
    // 2 → 3: vaults record their format (v2 vaults keep their vault.json).
    autoMigrations = [AutoMigration(from = 1, to = 2), AutoMigration(from = 2, to = 3)],
)
@TypeConverters(Converters::class)
abstract class AppDatabase : RoomDatabase() {
    abstract fun accounts(): AccountDao
    abstract fun pairs(): FolderPairDao
    abstract fun fileStates(): FileStateDao
    abstract fun runs(): SyncRunDao
    abstract fun events(): SyncEventDao
    abstract fun conflicts(): ConflictDao
    abstract fun trash(): TrashDao
    abstract fun nodes(): DriveNodeDao
    abstract fun uploadSessions(): UploadSessionDao
    abstract fun pins(): PinDao
    abstract fun vaults(): VaultDao
    abstract fun blockedPlans(): BlockedPlanDao

    companion object {
        fun build(context: Context): AppDatabase =
            // No destructive fallback: a missing migration must fail loudly in testing, never wipe the
            // user's pairs and sync history (and with them the record of what is in sync).
            Room.databaseBuilder(context, AppDatabase::class.java, "drivesync.db")
                .build()
    }
}
