"""TC_REQ_0007_04 (target: unit) -> com.almodhayan.drivesync.crypto.EncryptedRemoteFsTest.download_returnsThePlaintext"""
from _gate import gradle_unit


def test_TC_REQ_0007_04():
    gradle_unit("com.almodhayan.drivesync.crypto.EncryptedRemoteFsTest", "download_returnsThePlaintext")
