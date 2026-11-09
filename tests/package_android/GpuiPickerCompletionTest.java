package dev.gpui.mobile;

import java.lang.reflect.*;
import java.io.File;
import java.util.ArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.atomic.AtomicReference;

/** Exercises the production completion gate; not an Activity lifecycle/device simulation. */
public final class GpuiPickerCompletionTest {
    public static void main(String[] args) throws Exception {
        Class<?> request = Class.forName("dev.gpui.mobile.GpuiPickerActivity$Request");
        Constructor<?> constructor = request.getDeclaredConstructors()[0];
        constructor.setAccessible(true);
        Method complete = GpuiPickerActivity.class.getDeclaredMethod("complete", request, ArrayList.class, RuntimeException.class);
        complete.setAccessible(true);
        Field activeField = GpuiPickerActivity.class.getDeclaredField("ACTIVE");
        activeField.setAccessible(true);
        @SuppressWarnings("unchecked") AtomicReference<Object> active = (AtomicReference<Object>)activeField.get(null);
        Field latchField = request.getDeclaredField("latch"); latchField.setAccessible(true);
        Field errorField = request.getDeclaredField("error"); errorField.setAccessible(true);
        Object first = constructor.newInstance(null, true, null);
        Object second = constructor.newInstance(null, true, null);
        if (!active.compareAndSet(null, first)) throw new AssertionError("Slot not empty");
        CountDownLatch latch = (CountDownLatch)latchField.get(first);
        AtomicReference<Throwable> failure = new AtomicReference<>();
        Thread waiter = new Thread(() -> {
            try {
                latch.await();
                if (!active.compareAndSet(null, second)) throw new AssertionError("Active slot was not cleared before signalling");
            } catch (Throwable error) { failure.set(error); }
        });
        waiter.start();
        RuntimeException closed = new IllegalStateException("Proxy finishing teardown");
        complete.invoke(null, first, null, closed);
        waiter.join(2000);
        if (waiter.isAlive()) { waiter.interrupt(); waiter.join(); throw new AssertionError("Waiter stranded"); }
        if (failure.get() != null) throw new AssertionError(failure.get());
        if (errorField.get(first) != closed) throw new AssertionError("Terminal error lost");
        File directory = java.nio.file.Files.createTempDirectory("gpui-late-import-test-").toFile();
        File imported = new File(directory, "selected.txt");
        try {
            java.nio.file.Files.write(imported.toPath(), new byte[]{1});
            ArrayList<String> late = new ArrayList<>(); late.add(imported.getAbsolutePath());
            complete.invoke(null, first, late, null);
            if (directory.exists()) throw new AssertionError("Late import was not reclaimed");
            if (active.get() != second) throw new AssertionError("Late A completion affected B");
            complete.invoke(null, second, null, null);
            if (active.get() != null) throw new AssertionError("Second request did not finish");
        } finally {
            java.nio.file.Files.deleteIfExists(imported.toPath());
            java.nio.file.Files.deleteIfExists(directory.toPath());
            active.compareAndSet(first, null); active.compareAndSet(second, null);
        }
        System.out.println("PASS: request terminal gate, release-before-signal, late import reclamation");
    }
}
