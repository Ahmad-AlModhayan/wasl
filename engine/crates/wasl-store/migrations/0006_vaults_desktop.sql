-- Wasl schema v6: vaults on a computer (docs/ENCRYPTION.md §6 and §11.1). A vault's row holds no
-- secret: its vault.json (salt, cost, key check) and the Drive folder it was found in or made in.
-- While the vault is unlocked on this device its key material sits in the OS keychain, never here,
-- and the passphrase is kept nowhere.
ALTER TABLE vaults ADD COLUMN manifest_json   TEXT NOT NULL DEFAULT '';
ALTER TABLE vaults ADD COLUMN account_id      TEXT;
ALTER TABLE vaults ADD COLUMN drive_folder_id TEXT;
ALTER TABLE vaults ADD COLUMN drive_id        TEXT;
ALTER TABLE vaults ADD COLUMN remote_path     TEXT NOT NULL DEFAULT '';
ALTER TABLE vaults ADD COLUMN created_at      INTEGER NOT NULL DEFAULT 0;
