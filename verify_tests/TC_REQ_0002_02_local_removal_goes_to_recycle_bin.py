"""TC_REQ_0002_02 (target: unit) -> com.almodhayan.drivesync.fs.LocalFsTrashTest.moveToTrash_keepsBytesAndRemovesOriginal"""
from _gate import gradle_unit


def test_TC_REQ_0002_02():
    gradle_unit("com.almodhayan.drivesync.fs.LocalFsTrashTest", "moveToTrash_keepsBytesAndRemovesOriginal")
