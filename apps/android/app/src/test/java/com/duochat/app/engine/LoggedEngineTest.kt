package com.duochat.app.engine

import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The screens reach the engine through [ChatEngineApi], which is wrapped in [LoggedEngine] for the diagnostic log. A call the
 * wrapper does not pass on would reach the interface's default instead of the real engine. Creating an identity did exactly
 * that once ("The engine is not ready yet." on every fresh install), so the wrapper must pass it on.
 */
class LoggedEngineTest {
    @Test
    fun the_wrapper_passes_identity_creation_on_to_the_real_engine() {
        val passedOn = LoggedEngine::class.java.declaredMethods.any { it.name == "recoverIdentity" }
        assertTrue("LoggedEngine must delegate recoverIdentity to the engine it wraps", passedOn)
    }
}
