"""TC_REQ_0002_03 (target: unit) -> com.almodhayan.drivesync.fs.LocalFsTrashTest.restore_putsTheFileBack"""
from _gate import gradle_unit


def test_TC_REQ_0002_03():
    gradle_unit("com.almodhayan.drivesync.fs.LocalFsTrashTest", "restore_putsTheFileBack")
