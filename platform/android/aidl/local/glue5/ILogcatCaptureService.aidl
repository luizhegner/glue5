package local.glue5;

import android.os.ParcelFileDescriptor;

interface ILogcatCaptureService {
    ParcelFileDescriptor captureLogcat() = 1;
    void destroy() = 16777114;
}
