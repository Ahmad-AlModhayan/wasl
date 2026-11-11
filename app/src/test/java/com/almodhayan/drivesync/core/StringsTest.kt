package com.almodhayan.drivesync.core

import com.almodhayan.drivesync.R
import org.junit.Assert.assertEquals
import org.junit.Test

class StringsTest {
    @Test fun `unit tests read the English resources`() {
        assertEquals("OK", Strings.get(R.string.common_ok))
        assertEquals("Wasl", Strings.get(R.string.app_name))
    }
}
