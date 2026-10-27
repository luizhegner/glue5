package local.glue5;

import android.os.ParcelFileDescriptor;
import android.os.RemoteException;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;

/** Runs logcat with the shell identity granted by Shizuku and streams it over a pipe. */
public final class ShizukuLogcatService extends ILogcatCaptureService.Stub {
    @Override
    public ParcelFileDescriptor captureLogcat() throws RemoteException {
        final ParcelFileDescriptor[] pipe;
        try {
            pipe = ParcelFileDescriptor.createPipe();
        } catch (IOException error) {
            throw new RemoteException("Could not create logcat pipe: " + error);
        }

        Thread worker = new Thread(() -> streamLogcat(pipe[1]), "glue5-shizuku-logcat");
        worker.setDaemon(true);
        worker.start();
        return pipe[0];
    }

    private static void streamLogcat(ParcelFileDescriptor writeEnd) {
        Process process = null;
        try (OutputStream output = new ParcelFileDescriptor.AutoCloseOutputStream(writeEnd)) {
            try {
                process = new ProcessBuilder("/system/bin/logcat", "-d", "-b", "all", "-v", "threadtime")
                        .redirectErrorStream(true)
                        .start();
                try (InputStream input = process.getInputStream()) {
                    byte[] buffer = new byte[64 * 1024];
                    int read;
                    while ((read = input.read(buffer)) != -1) {
                        output.write(buffer, 0, read);
                    }
                }
                int exit = process.waitFor();
                if (exit != 0) {
                    output.write(("\nGLUE5_LOGCAT_ERROR: logcat exited with code " + exit + "\n")
                            .getBytes(StandardCharsets.UTF_8));
                }
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                output.write(("\nGLUE5_LOGCAT_ERROR: interrupted: " + error + "\n")
                        .getBytes(StandardCharsets.UTF_8));
            } catch (IOException error) {
                output.write(("\nGLUE5_LOGCAT_ERROR: " + error + "\n")
                        .getBytes(StandardCharsets.UTF_8));
            }
        } catch (IOException ignored) {
            // The app closed the read end, or storage/capture was abandoned.
        } finally {
            if (process != null) {
                process.destroy();
            }
        }
    }

    /** Shizuku calls this reserved method when the user service is removed. */
    @Override
    public void destroy() {
        System.exit(0);
    }
}
