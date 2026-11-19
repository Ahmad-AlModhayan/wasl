-- The kind of failure a run ended with (`wasl_proto::ErrorKind`, as its `as_str` names it: AUTH,
-- NETWORK, ...), so the history words an older failed run from its cause and not with the generic
-- sentence (issue #57). NULL for a run that did not fail, and for every run stored before this
-- migration: the old rows keep the generic sentence, since the kind was never kept.
ALTER TABLE runs ADD COLUMN error_kind TEXT;
