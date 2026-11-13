package org.cashudevkit.sbomconsumer;

import androidx.test.ext.junit.runners.AndroidJUnit4;
import org.junit.Test;
import org.junit.runner.RunWith;
import static org.junit.Assert.assertTrue;

@RunWith(AndroidJUnit4.class)
public class NativeLoadTest {
    @Test
    public void callsPackagedNativeLibrary() throws Exception {
        Object mnemonic = Class.forName("org.cashudevkit.Cdk_ffiKt")
            .getMethod("generateMnemonic").invoke(null);
        assertTrue(mnemonic instanceof String && !((String) mnemonic).isEmpty());
    }
}
