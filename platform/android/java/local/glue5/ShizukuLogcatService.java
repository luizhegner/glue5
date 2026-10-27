package local.glue5;

import android.os.ParcelFileDescriptor;
import android.os.RemoteException;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;

/** Runs logcat with the shell identity granted by Shizuku and streams it over a pipe. */
public final class ShizukuLogcatService extends ILogcatCaptureService.Stub {
    private final Object liveLock = new Object();
    private volatile Process liveProcess;
    private volatile boolean liveStopRequested;
    private volatile ParcelFileDescriptor liveWriteEnd;

    /** Snapshot used by the previous-session recovery flow. */
    @Override
    public ParcelFileDescriptor captureLogcat() throws RemoteException {
        final ParcelFileDescriptor[] pipe;
        try {
            pipe = ParcelFileDescriptor.createPipe();
        } catch (IOException error) {
            throw new RemoteException("Could not create logcat pipe: " + error);
        }

        Thread worker = new Thread(() -> streamSnapshot(pipe[1]), "glue5-shizuku-logcat-snapshot");
        worker.setDaemon(true);
        worker.start();
        return pipe[0];
    }

    /** Start following the logcat ring buffer until stopLogcat() is called. */
    @Override
    public ParcelFileDescriptor startLogcat() throws RemoteException {
        final ParcelFileDescriptor[] pipe;
        try {
            pipe = ParcelFileDescriptor.createPipe();
        } catch (IOException error) {
            throw new RemoteException("Could not create live logcat pipe: " + error);
        }

        synchronized (liveLock) {
            if (liveWriteEnd != null) {
                try {
                    pipe[0].close();
                    pipe[1].close();
                } catch (IOException ignored) {
                    // The failed request owns no stream.
                }
                throw new RemoteException("A live logcat capture is already active");
            }
            liveStopRequested = false;
            liveWriteEnd = pipe[1];
        }

        Thread worker = new Thread(() -> streamLive(pipe[1]), "glue5-shizuku-logcat-live");
        worker.setDaemon(true);
        worker.start();
        return pipe[0];
    }

    /** Stop the live logcat process; closing its stdout produces EOF on the app's pipe. */
    @Override
    public void stopLogcat() {
        Process process;
        synchronized (liveLock) {
            liveStopRequested = true;
            process = liveProcess;
        }
        if (process != null) {
            process.destroy();
        }
    }

    private static void streamSnapshot(ParcelFileDescriptor writeEnd) {
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

    private void streamLive(ParcelFileDescriptor writeEnd) {
        Process process = null;
        try (OutputStream output = new ParcelFileDescriptor.AutoCloseOutputStream(writeEnd)) {
            try {
                process = new ProcessBuilder("/system/bin/logcat", "-b", "all", "-v", "threadtime", "-T", "1")
                        .redirectErrorStream(true)
                        .start();
                synchronized (liveLock) {
                    liveProcess = process;
                    if (liveStopRequested) {
                        process.destroy();
                    }
                }
                try (InputStream input = process.getInputStream()) {
                    byte[] buffer = new byte[32 * 1024];
                    int read;
                    while ((read = input.read(buffer)) != -1) {
                        output.write(buffer, 0, read);
                        output.flush();
                    }
                }
                int exit = process.waitFor();
                if (exit != 0 && !liveStopRequested) {
                    output.write(("\nGLUE5_LOGCAT_ERROR: live logcat exited with code " + exit + "\n")
                            .getBytes(StandardCharsets.UTF_8));
                }
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                if (!liveStopRequested) {
                    output.write(("\nGLUE5_LOGCAT_ERROR: live logcat interrupted: " + error + "\n")
                            .getBytes(StandardCharsets.UTF_8));
                }
            } catch (IOException error) {
                if (!liveStopRequested) {
                    try {
                        output.write(("\nGLUE5_LOGCAT_ERROR: live logcat: " + error + "\n")
                                .getBytes(StandardCharsets.UTF_8));
                    } catch (IOException ignored) {
                        // The app closed the read end.
                    }
                }
            }
        } catch (IOException ignored) {
            // The app closed the read end, or storage/capture was abandoned.
        } finally {
            if (process != null) {
                process.destroy();
            }
            synchronized (liveLock) {
                liveProcess = null;
                liveWriteEnd = null;
            }
        }
    }

    /** Shizuku calls this reserved method when the user service is removed. */
    @Override
    public void destroy() {
        stopLogcat();
        System.exit(0);
    }
}
