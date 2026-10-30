package local.glue5;

import android.os.ParcelFileDescriptor;

interface ILogcatCaptureService {
    ParcelFileDescriptor captureLogcat(int pid) = 1;
    ParcelFileDescriptor startLogcat(int pid) = 2;
    void stopLogcat() = 3;
    void destroy() = 16777114;
}
