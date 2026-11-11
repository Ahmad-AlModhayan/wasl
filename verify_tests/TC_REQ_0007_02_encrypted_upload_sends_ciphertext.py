"""TC_REQ_0007_02 (target: unit) -> com.almodhayan.drivesync.crypto.EncryptedRemoteFsTest.upload_sendsCiphertextOnly"""
from _gate import gradle_unit


def test_TC_REQ_0007_02():
    gradle_unit("com.almodhayan.drivesync.crypto.EncryptedRemoteFsTest", "upload_sendsCiphertextOnly")
