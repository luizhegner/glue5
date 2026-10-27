package local.glue5;

import android.os.ParcelFileDescriptor;

interface ILogcatCaptureService {
    ParcelFileDescriptor captureLogcat();
    void destroy() = 16777114;
}
