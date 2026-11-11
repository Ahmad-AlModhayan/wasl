"""TC_REQ_0007_03 (target: unit) -> com.almodhayan.drivesync.crypto.VaultNamesTest.decryptReturnsNullForTamperedName"""
from _gate import gradle_unit


def test_TC_REQ_0007_03():
    gradle_unit("com.almodhayan.drivesync.crypto.VaultNamesTest", "decryptReturnsNullForTamperedName")
