"""TC_REQ_0007_01 (target: unit) -> com.almodhayan.drivesync.crypto.EncryptedRemoteFsTest.upload_sendsAnEncryptedName"""
from _gate import gradle_unit


def test_TC_REQ_0007_01():
    gradle_unit("com.almodhayan.drivesync.crypto.EncryptedRemoteFsTest", "upload_sendsAnEncryptedName")
