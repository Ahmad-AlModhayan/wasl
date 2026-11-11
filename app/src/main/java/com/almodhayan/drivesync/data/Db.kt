package com.almodhayan.drivesync.data

import com.almodhayan.drivesync.data.db.AccountDao
import com.almodhayan.drivesync.data.db.AccountEntity
import com.almodhayan.drivesync.data.db.AppDatabase
import com.almodhayan.drivesync.engine.EngineData
import com.almodhayan.drivesync.engine.WaslRuntime
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.withContext
import javax.inject.Inject
import javax.inject.Singleton

/**
 * The app's data, by owner. Pairs, baselines, runs, conflicts, the bin and stopped plans live in
 * the engine (libwasl); accounts, the Drive browser cache, pins and vault metadata stay in Room
 * until their features move into the engine (files on demand, M5).
 */
@Singleton
class Db @Inject constructor(
    private val room: AppDatabase,
    private val engine: EngineData,
    private val runtime: WaslRuntime,
) {
    fun pairs() = engine.pairs
    fun fileStates() = engine.fileStates
    fun runs() = engine.runs
    fun events() = engine.events
    fun conflicts() = engine.conflicts
    fun trash() = engine.trash
    fun blockedPlans() = engine.blockedPlans

    fun accounts(): AccountDao = accounts
    fun nodes() = room.nodes()
    fun uploadSessions() = room.uploadSessions()
    fun pins() = room.pins()
    fun vaults() = room.vaults()

    /** Accounts from Room, with the sign-in problem the engine last hit during a sync. */
    private val accounts: AccountDao = object : AccountDao by room.accounts() {
        private suspend fun AccountEntity.withEngineError(): AccountEntity = withContext(Dispatchers.IO) {
            val err = runCatching { runtime.engine.accountAuthError(id) }.getOrNull()
            if (err != null) copy(authError = err) else this@withEngineError
        }

        override fun observeAll(): Flow<List<AccountEntity>> =
            combine(room.accounts().observeAll(), runtime.changes) { list, _ -> list.map { it.withEngineError() } }.flowOn(Dispatchers.IO)

        override fun observe(id: String): Flow<AccountEntity?> =
            combine(room.accounts().observe(id), runtime.changes) { a, _ -> a?.withEngineError() }.flowOn(Dispatchers.IO)

        override suspend fun getAll(): List<AccountEntity> = room.accounts().getAll().map { it.withEngineError() }

        override suspend fun get(id: String): AccountEntity? = room.accounts().get(id)?.withEngineError()

        override suspend fun upsert(a: AccountEntity) {
            room.accounts().upsert(a)
            // The engine keys Drive caches and sign-in errors by account id.
            withContext(Dispatchers.IO) { runCatching { runtime.engine.setAccountEmail(a.id, a.id) } }
        }
    }
}
