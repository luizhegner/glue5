package local.glue5;

import android.content.Context;
import android.os.Build;
import android.view.View;
import android.view.WindowInsets;

import org.libsdl.app.SDLSurface;

/**
 * SDL's surface, which also tells the app how much of it the on-screen
 * keyboard covers (Android 11 and later): the app's pages end above it, as
 * the window does not pan for it there (Glue5Activity).
 */
public class Glue5Surface extends SDLSurface {
    public Glue5Surface(Context context) {
        super(context);
    }

    @Override
    public WindowInsets onApplyWindowInsets(View v, WindowInsets insets) {
        if (Build.VERSION.SDK_INT >= 30 /* Android 11 (R) */) {
            // Before SDL's, which queues the keyboard's shown and hidden
            // events: the app has the height when it gets them.
            onKeyboardInset(insets.getInsets(WindowInsets.Type.ime()).bottom);
        }
        return super.onApplyWindowInsets(v, insets);
    }

    /** The keyboard's height over the surface's bottom, in pixels (keyboard.rs). */
    private static native void onKeyboardInset(int bottom);
}
