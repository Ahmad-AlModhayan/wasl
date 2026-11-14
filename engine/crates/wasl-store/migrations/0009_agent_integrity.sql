-- Wasl schema v9 (security review 2026-10): agent changes keep proof of what they saved and of who
-- decided (docs/AGENT_CHANGES.md).

-- The BLAKE3 of each saved earlier version (hex), taken while it was copied. A revert or diff checks
-- it, so a copy changed in the agent folder afterwards is never put back. Empty for copies saved
-- before v9.
ALTER TABLE agent_saved ADD COLUMN blake3 TEXT;

-- Which client accepted a changeset ("cli", "desktop", …; "unknown" when the client did not say).
ALTER TABLE agent_changesets ADD COLUMN decided_by TEXT;
