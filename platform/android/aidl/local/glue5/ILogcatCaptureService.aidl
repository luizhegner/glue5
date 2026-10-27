package local.glue5;

import android.os.ParcelFileDescriptor;

interface ILogcatCaptureService {
    ParcelFileDescriptor captureLogcat() = 1;
    ParcelFileDescriptor startLogcat() = 2;
    void stopLogcat() = 3;
    void destroy() = 16777114;
}
