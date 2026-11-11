package com.almodhayan.drivesync.di

import android.content.Context
import com.almodhayan.drivesync.data.db.*
import com.almodhayan.drivesync.drive.DriveAuth
import com.almodhayan.drivesync.drive.DriveEndpoints
import com.almodhayan.drivesync.drive.TokenProvider
import dagger.Binds
import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.android.qualifiers.ApplicationContext
import dagger.hilt.components.SingletonComponent
import dagger.multibindings.Multibinds
import kotlinx.serialization.json.Json
import okhttp3.OkHttpClient
import java.util.concurrent.TimeUnit
import javax.inject.Singleton

@Module
@InstallIn(SingletonComponent::class)
abstract class BindingsModule {
    @Binds abstract fun tokenProvider(impl: DriveAuth): TokenProvider
}

@Module
@InstallIn(SingletonComponent::class)
object AppModule {
    @Provides @Singleton
    fun json(): Json = Json { ignoreUnknownKeys = true; isLenient = true; encodeDefaults = true; explicitNulls = false }

    @Provides @Singleton
    fun okHttp(): OkHttpClient = OkHttpClient.Builder()
        .connectTimeout(30, TimeUnit.SECONDS)
        .readTimeout(120, TimeUnit.SECONDS)
        .writeTimeout(120, TimeUnit.SECONDS)
        .retryOnConnectionFailure(true)
        .build()

    @Provides @Singleton
    fun endpoints(): DriveEndpoints = DriveEndpoints()

    @Provides @Singleton
    fun database(@ApplicationContext ctx: Context): AppDatabase = AppDatabase.build(ctx)

    // Engine-owned data comes from libwasl; the rest from Room (see data/Db.kt).
    @Provides fun accountDao(db: com.almodhayan.drivesync.data.Db): AccountDao = db.accounts()
    @Provides fun folderPairDao(db: com.almodhayan.drivesync.data.Db): FolderPairDao = db.pairs()
    @Provides fun fileStateDao(db: com.almodhayan.drivesync.data.Db): FileStateDao = db.fileStates()
    @Provides fun syncRunDao(db: com.almodhayan.drivesync.data.Db): SyncRunDao = db.runs()
    @Provides fun syncEventDao(db: com.almodhayan.drivesync.data.Db): SyncEventDao = db.events()
    @Provides fun conflictDao(db: com.almodhayan.drivesync.data.Db): ConflictDao = db.conflicts()
    @Provides fun trashDao(db: com.almodhayan.drivesync.data.Db): TrashDao = db.trash()
    @Provides fun driveNodeDao(db: AppDatabase): DriveNodeDao = db.nodes()
    @Provides fun uploadSessionDao(db: AppDatabase): UploadSessionDao = db.uploadSessions()
    @Provides fun pinDao(db: AppDatabase): PinDao = db.pins()
    @Provides fun vaultDao(db: AppDatabase): VaultDao = db.vaults()
    @Provides fun blockedPlanDao(db: com.almodhayan.drivesync.data.Db): BlockedPlanDao = db.blockedPlans()
}
