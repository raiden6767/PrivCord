package com.duochat.app.engine

import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The screens reach the engine through [ChatEngineApi], which is wrapped in [LoggedEngine] for the diagnostic log. A member of
 * the interface that has a default body is not forwarded by `ChatEngineApi by inner`: the wrapper must override it, or the call
 * reaches the default instead of the real engine. That is how "Create identity" once answered "The engine is not ready yet." on
 * every fresh install, and how "load earlier messages" did nothing. So every member of the interface must be declared on the
 * wrapper itself.
 */
class LoggedEngineTest {
    @Test
    fun every_member_of_the_engine_interface_is_declared_on_the_wrapper() {
        val declared = LoggedEngine::class.java.declaredMethods.map { it.name }.toSet()
        // (Static and synthetic members are Kotlin's own helpers for default arguments, not part of what the screens call.)
        val missing = ChatEngineApi::class.java.methods
            .filter { !it.isSynthetic && !java.lang.reflect.Modifier.isStatic(it.modifiers) }
            .map { it.name }
            .filter { it !in declared }
            .toSortedSet()
        assertTrue("LoggedEngine does not declare (so does not pass on): $missing", missing.isEmpty())
    }
}
