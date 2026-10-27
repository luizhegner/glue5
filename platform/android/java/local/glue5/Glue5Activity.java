package local.glue5;

import android.content.ActivityNotFoundException;
import android.content.ClipData;
import android.content.Context;
import android.content.ClipboardManager;
import android.content.ContentResolver;
import android.content.Intent;
import android.content.SharedPreferences;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.os.Looper;
import android.system.ErrnoException;
import android.util.Log;
import android.system.Os;
import android.provider.DocumentsContract;
import android.provider.Settings;
import android.widget.Toast;

import org.libsdl.app.SDLActivity;

import java.io.BufferedInputStream;
import java.io.BufferedOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.UUID;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

/** The app's activity: SDL's, with our library (libglue5.so) as the main one. */
public class Glue5Activity extends SDLActivity {
    private static final int IMPORT_TREE = 4105;
    private static final int GAME_TREE = 4106;
    private static final int ALL_FILES = 4107;
    private static final String IMPORT_PREFS = "glue5-import";
    private static final String PENDING_KIND = "kind";
    private static final String PENDING_URI = "uri";
    /** The kind that asks for the game's own folder, rather than files to copy in. */
    private static final String GAME_KIND = "game";
    /** Where Rust reads the folder chosen for the game, or why there is none. */
    private static final String GAME_PATH = "game-path";
    private static final String GAME_ERROR = "game-error";
    private static final AtomicBoolean COPYING = new AtomicBoolean();
    private static final ExecutorService IMPORTS = Executors.newSingleThreadExecutor();
    private static volatile Glue5Activity active;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        // The same switches as a desktop run, RUST_LOG and GLUE5_*: string
        // extras of the launch intent (`mise run android:run` passes them),
        // set in the environment before SDL starts the library's main.
        Bundle extras = getIntent().getExtras();
        if (extras != null) {
            for (String key : extras.keySet()) {
                String value = extras.getString(key);
                if (value != null && (key.equals("RUST_LOG") || key.startsWith("GLUE5_"))) {
                    try {
                        Os.setenv(key, value, true);
                    } catch (ErrnoException e) {
                        // Left unset.
                    }
                }
            }
        }
        super.onCreate(savedInstanceState);
        active = this;
        resumePendingImport();
    }

    @Override
    protected String[] getLibraries() {
        return new String[] { "SDL3", "glue5" };
    }

    /** Called from Rust.  A tree, rather than arbitrary storage permission,
     * gives the app access only to the folder the player selects. */
    public static boolean startImport(String kind) {
        Glue5Activity activity = active;
        if (activity == null
                || (!kind.equals("save") && !kind.equals("resources") && !kind.equals(GAME_KIND))) {
            return false;
        }
        activity.runOnUiThread(() -> {
            try {
                activity.openImportTree(kind);
            } catch (ActivityNotFoundException error) {
                Log.e("glue5", "no Android folder picker available for " + kind, error);
                if (GAME_KIND.equals(kind)) {
                    activity.writeGameText(GAME_ERROR, "Android could not open a folder picker. Install or enable a Files app, then try Find files again.");
                } else {
                    Toast.makeText(activity, "Could not open a folder picker", Toast.LENGTH_LONG).show();
                }
            } catch (RuntimeException error) {
                Log.e("glue5", "could not start folder picker for " + kind, error);
                if (GAME_KIND.equals(kind)) {
                    activity.writeGameText(GAME_ERROR, "Could not start the folder picker: " + error);
                } else {
                    Toast.makeText(activity, "Could not start the folder picker", Toast.LENGTH_LONG).show();
                }
            }
        });
        return true;
    }

    /** Saves and copies a bounded support report. Fatal game failures call this before the activity exits. */
    public static void copyImportDiagnostics(String report) {
        Glue5Activity activity = active;
        if (activity == null) {
            return;
        }
        String abi = Build.SUPPORTED_ABIS.length == 0 ? "unknown" : Build.SUPPORTED_ABIS[0];
        String device = "Android: " + Build.MANUFACTURER + " " + Build.MODEL
                + " (" + Build.VERSION.RELEASE + ", API " + Build.VERSION.SDK_INT + ", " + abi + ")\n";
        String complete = device + report;
        Runnable publish = () -> {
            boolean saved = false;
            boolean copied = false;
            try {
                File external = activity.getExternalFilesDir(null);
                if (external != null) {
                    writeText(new File(external, "glue5-support-report.txt"), complete);
                    saved = true;
                }
            } catch (IOException | RuntimeException error) {
                Log.e("glue5", "cannot save support report", error);
            }
            try {
                ClipboardManager clipboard = (ClipboardManager) activity.getSystemService(Context.CLIPBOARD_SERVICE);
                if (clipboard != null) {
                    clipboard.setPrimaryClip(ClipData.newPlainText("GLUE 5 support diagnostics", complete));
                    copied = true;
                }
            } catch (RuntimeException error) {
                Log.e("glue5", "cannot copy support report", error);
            }
            String message = copied
                    ? (saved ? "Support report saved and copied" : "Support report copied")
                    : (saved ? "Support report saved; clipboard unavailable" : "Could not save support report");
            try {
                Toast.makeText(activity, message, Toast.LENGTH_LONG).show();
            } catch (RuntimeException error) {
                Log.e("glue5", "cannot show support report result", error);
            }
        };
        if (Looper.myLooper() == Looper.getMainLooper()) {
            publish.run();
            return;
        }
        CountDownLatch finished = new CountDownLatch(1);
        activity.runOnUiThread(() -> {
            try {
                publish.run();
            } finally {
                finished.countDown();
            }
        });
        try {
            if (!finished.await(3, TimeUnit.SECONDS)) {
                Log.e("glue5", "timed out saving/copying support report before activity exit");
            }
        } catch (InterruptedException error) {
            Thread.currentThread().interrupt();
        }
    }

    private void openImportTree(String kind) {
        SharedPreferences prefs = getSharedPreferences(IMPORT_PREFS, MODE_PRIVATE);
        prefs.edit().putString(PENDING_KIND, kind).remove(PENDING_URI).apply();
        if (kind.equals(GAME_KIND)) {
            clearGamePick();
            openGameTree();
            return;
        }
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION);
        startActivityForResult(intent, IMPORT_TREE);
    }

    /**
     * The folder the game is played from, chosen rather than copied: its
     * files are the engine's 20 GB, read where they are.
     *
     * A document tree's grant reaches files through the provider, one binder
     * call an open, which the engine's reads (and the paths its VFS takes)
     * are not written for; and copying that much into the app's own folder
     * needs the room twice over.  So the folder is read by its path, which
     * takes all-files access: asked for first, and the picker goes on once
     * the player is back from the system's page for it.
     */
    private void openGameTree() {
        SharedPreferences prefs = getSharedPreferences(IMPORT_PREFS, MODE_PRIVATE);
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            // Android 10 has no all-files access to ask for, and an app
            // targeting this one gets no legacy storage either.
            prefs.edit().clear().apply();
            writeGameText(GAME_ERROR, "Android 10: copy the game folder into "
                    + "Android/data/local.glue5/files/, named game. This page looks again "
                    + "when the app comes back.");
            return;
        }
        if (!Environment.isExternalStorageManager()) {
            Toast.makeText(this, "Allow Glue 5 to access all files, then choose the game folder.", Toast.LENGTH_LONG).show();
            askAllFilesAccess();
            return;
        }
        prefs.edit().remove(PENDING_KIND).apply();
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION);
        try {
            startActivityForResult(intent, GAME_TREE);
        } catch (ActivityNotFoundException error) {
            Log.e("glue5", "no Android folder picker available for game files", error);
            writeGameText(GAME_ERROR, "Android could not open a folder picker. Install or enable a Files app, then try Find files again.");
        } catch (RuntimeException error) {
            Log.e("glue5", "could not start Android folder picker for game files", error);
            writeGameText(GAME_ERROR, "Could not start the Android folder picker: " + error);
        }
    }

    /** The system's page for this app's all-files access; it answers to
     * {@link #onResume}, not with a result.  The version check is here, and
     * not only at the call, for lint: the API is named in this method. */
    private void askAllFilesAccess() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            return;
        }
        Uri self = Uri.parse("package:" + getPackageName());
        Intent intent = new Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, self);
        try {
            startActivityForResult(intent, ALL_FILES);
        } catch (ActivityNotFoundException ignored) {
            // A system without that page: its general one, or nothing.
            try {
                startActivityForResult(new Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION), ALL_FILES);
            } catch (RuntimeException also) {
                getSharedPreferences(IMPORT_PREFS, MODE_PRIVATE).edit().clear().apply();
                writeGameText(GAME_ERROR, "Could not open Android's all-files access settings. Copy the game folder into Android/data/local.glue5/files/, named game.");
            }
        } catch (RuntimeException error) {
            getSharedPreferences(IMPORT_PREFS, MODE_PRIVATE).edit().clear().apply();
            writeGameText(GAME_ERROR, "Could not open Android's all-files access settings: " + error);
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        SharedPreferences prefs = getSharedPreferences(IMPORT_PREFS, MODE_PRIVATE);
        if (!GAME_KIND.equals(prefs.getString(PENDING_KIND, null))) {
            return;
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R && Environment.isExternalStorageManager()) {
            // Back from the system's page with it: on to the picker.
            openGameTree();
        } else {
            prefs.edit().clear().apply();
            writeGameText(GAME_ERROR, "Without all-files access the app cannot read a game folder "
                    + "of yours. Copy the game folder into Android/data/local.glue5/files/, named "
                    + "game, or play it from a computer that shares it.");
        }
    }

    /** The folder the player chose for the game, left for Rust to try. */
    private void gameTreeChosen(int resultCode, Intent data) {
        SharedPreferences prefs = getSharedPreferences(IMPORT_PREFS, MODE_PRIVATE);
        prefs.edit().clear().apply();
        if (resultCode != RESULT_OK || data == null || data.getData() == null) {
            writeGameText(GAME_ERROR, "No game folder was selected. Tap Find files and choose the folder containing the game.");
            return;
        }
        File dir;
        try {
            dir = treePath(data.getData());
        } catch (RuntimeException error) {
            writeGameText(GAME_ERROR, "Android could not read the selected folder: " + error);
            return;
        }
        if (dir == null) {
            writeGameText(GAME_ERROR, "That folder has no path of its own (a cloud one, perhaps). "
                    + "Choose a folder on this phone's storage.");
            return;
        }
        if (!dir.isDirectory() || !dir.canRead()) {
            writeGameText(GAME_ERROR, dir + " is not a readable folder. Grant all-files access and choose a folder on local device storage.");
            return;
        }
        // Whether it has the game is Rust's call, which knows this build's
        // layout; a folder it cannot read shows there too.
        writeGameText(GAME_PATH, dir.getAbsolutePath());
    }

    /**
     * Where a chosen tree is on the storage: the local provider's ids are
     * paths under a volume.  None for a provider with no path of its own
     * (a cloud one), whose files the engine could not read anyway.
     */
    private static File treePath(Uri tree) {
        if (!"com.android.externalstorage.documents".equals(tree.getAuthority())) {
            return null;
        }
        String id = DocumentsContract.getTreeDocumentId(tree);
        int colon = id.indexOf(':');
        if (colon <= 0) {
            return null;
        }
        String volume = id.substring(0, colon);
        String rest = id.substring(colon + 1);
        if ("primary".equals(volume)) {
            return new File(Environment.getExternalStorageDirectory(), rest);
        }
        // An SD card or a USB drive, by its volume's name.
        return new File("/storage/" + volume, rest);
    }

    /** Removes a stale picker result so canceling a new attempt cannot reuse an old folder. */
    private void clearGamePick() {
        File external = getExternalFilesDir(null);
        if (external == null) {
            return;
        }
        File imports = new File(external, "imports");
        new File(imports, GAME_PATH).delete();
        new File(imports, GAME_ERROR).delete();
    }

    /** Leaves an answer for Rust, which reads it while the start page is up. */
    private void writeGameText(String name, String text) {
        File external = getExternalFilesDir(null);
        if (external == null) {
            Log.e("glue5", "cannot record Find files result: external app directory is unavailable");
            return;
        }
        File imports = new File(external, "imports");
        if (!imports.isDirectory() && !imports.mkdirs() && !imports.isDirectory()) {
            Log.e("glue5", "cannot create the imports directory for Find files");
            return;
        }
        try {
            writeText(new File(imports, name), text);
            Log.i("glue5", "Find files result recorded: " + name);
        } catch (IOException error) {
            Log.e("glue5", "cannot record Find files result: " + name, error);
        }
    }

    @Override
    @SuppressWarnings("deprecation")
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode == GAME_TREE) {
            gameTreeChosen(resultCode, data);
            return;
        }
        if (requestCode == ALL_FILES) {
            // The system's page answers to onResume, not here; a result is
            // only another chance to see what the player decided.
            return;
        }
        if (requestCode != IMPORT_TREE) {
            return;
        }
        SharedPreferences prefs = getSharedPreferences(IMPORT_PREFS, MODE_PRIVATE);
        if (resultCode != RESULT_OK || data == null || data.getData() == null) {
            prefs.edit().clear().apply();
            return;
        }
        Uri tree = data.getData();
        try {
            getContentResolver().takePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION);
        } catch (SecurityException ignored) {
            // Some document providers do not offer a persistable grant. The
            // result can still be copied now while this process is alive.
        }
        prefs.edit().putString(PENDING_URI, tree.toString()).apply();
        resumePendingImport();
    }

    /** If Android killed the renderer while the picker was visible, the URI
     * and chosen kind are still here when the activity is made again. */
    private void resumePendingImport() {
        SharedPreferences prefs = getSharedPreferences(IMPORT_PREFS, MODE_PRIVATE);
        String kind = prefs.getString(PENDING_KIND, null);
        String saved = prefs.getString(PENDING_URI, null);
        if (kind == null || saved == null || !COPYING.compareAndSet(false, true)) {
            return;
        }
        IMPORTS.execute(() -> copyPendingImport(kind, Uri.parse(saved)));
    }

    private void copyPendingImport(String kind, Uri tree) {
        String token = kind + "-" + UUID.randomUUID();
        String selectedName = displayName(tree);
        String selectedId;
        try {
            selectedId = DocumentsContract.getTreeDocumentId(tree);
        } catch (Exception error) {
            selectedId = "";
        }
        Log.i("glue5", "import: copying kind=" + kind + " selected-folder=" + selectedName + " tree-id=" + selectedId);
        File external = getExternalFilesDir(null);
        if (external == null) {
            Log.e("glue5", "import: app external files directory is unavailable");
            COPYING.set(false);
            return;
        }
        File root = new File(external, "imports");
        File partial = new File(new File(root, "partial"), token);
        try {
            deleteTree(partial);
            if (!partial.mkdirs() && !partial.isDirectory()) {
                throw new IOException("cannot create import staging folder");
            }
            writeText(new File(partial, ".glue5-source-name"), selectedName);
            writeText(new File(partial, ".glue5-source-document-id"), selectedId);
            copyTree(tree, partial);
            File ready = new File(new File(root, "ready"), token);
            if (!ready.getParentFile().mkdirs() && !ready.getParentFile().isDirectory()) {
                throw new IOException("cannot create completed imports folder");
            }
            if (!partial.renameTo(ready)) {
                throw new IOException("cannot finalize imported folder");
            }
            Log.i("glue5", "import: copied selected-folder=" + selectedName + " tree-id=" + selectedId
                    + " staging=" + ready.getAbsolutePath());
            getSharedPreferences(IMPORT_PREFS, MODE_PRIVATE).edit().clear().apply();
        } catch (Exception error) {
            Log.e("glue5", "import: failed kind=" + kind + " selected-folder=" + selectedName
                    + " tree-id=" + selectedId, error);
            try {
                File errors = new File(root, "errors");
                errors.mkdirs();
                writeText(new File(errors, token + ".txt"), error + " (selected folder: " + selectedName
                        + "; document id: " + selectedId + ")");
            } catch (IOException ignored) {
            }
            deleteTree(partial);
            getSharedPreferences(IMPORT_PREFS, MODE_PRIVATE).edit().clear().apply();
        } finally {
            COPYING.set(false);
        }
    }

    /** Copies descendants only.  Document providers supply opaque IDs, so
     * this cannot follow a path outside the folder selected by the player. */
    private void copyTree(Uri tree, File destination) throws IOException {
        ContentResolver resolver = getContentResolver();
        String parentId = DocumentsContract.getTreeDocumentId(tree);
        Uri children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, parentId);
        String[] columns = {
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE
        };
        try (Cursor cursor = resolver.query(children, columns, null, null, null)) {
            if (cursor == null) {
                throw new IOException("the selected folder cannot be read");
            }
            int id = cursor.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_DOCUMENT_ID);
            int name = cursor.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_DISPLAY_NAME);
            int mime = cursor.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_MIME_TYPE);
            while (cursor.moveToNext()) {
                String childName = safeName(cursor.getString(name));
                String childId = cursor.getString(id);
                String type = cursor.getString(mime);
                Uri child = DocumentsContract.buildDocumentUriUsingTree(tree, childId);
                File out = new File(destination, childName);
                if (DocumentsContract.Document.MIME_TYPE_DIR.equals(type)) {
                    if (!out.mkdirs() && !out.isDirectory()) {
                        throw new IOException("cannot create " + childName);
                    }
                    copyTree(child, out);
                } else {
                    copyFile(resolver, child, out);
                }
            }
        }
    }

    private String displayName(Uri tree) {
        String documentId = null;
        try {
            documentId = DocumentsContract.getTreeDocumentId(tree);
            Uri document = DocumentsContract.buildDocumentUriUsingTree(tree, documentId);
            try (Cursor cursor = getContentResolver().query(document,
                    new String[] { DocumentsContract.Document.COLUMN_DISPLAY_NAME }, null, null, null)) {
                if (cursor != null && cursor.moveToFirst()) {
                    return safeName(cursor.getString(0));
                }
            }
        } catch (Exception ignored) {
            // Some providers do not return metadata for the tree root.
        }
        // The external-storage provider encodes the selected folder's path in
        // its document ID (for example primary:.../Profiles/0F74F4C4).
        if (documentId != null
                && "com.android.externalstorage.documents".equals(tree.getAuthority())) {
            String path = documentId;
            int volume = path.indexOf(':');
            if (volume >= 0) {
                path = path.substring(volume + 1);
            }
            path = path.replace('\\', '/');
            while (path.endsWith("/")) {
                path = path.substring(0, path.length() - 1);
            }
            String name = path.substring(path.lastIndexOf('/') + 1);
            try {
                return safeName(name);
            } catch (IOException ignored) {
                // Rust refuses to install if no genuine profile name is found.
            }
        }
        return "";
    }

    private static String safeName(String value) throws IOException {
        if (value == null || value.isEmpty() || value.equals(".") || value.equals("..")
                || value.indexOf('/') >= 0 || value.indexOf('\\') >= 0) {
            throw new IOException("unsafe file name from document provider");
        }
        return value;
    }

    private static void copyFile(ContentResolver resolver, Uri source, File target) throws IOException {
        InputStream raw = resolver.openInputStream(source);
        if (raw == null) {
            throw new IOException("cannot open selected file");
        }
        try (InputStream input = new BufferedInputStream(raw);
             OutputStream output = new BufferedOutputStream(new FileOutputStream(target))) {
            byte[] buffer = new byte[64 * 1024];
            for (int read; (read = input.read(buffer)) >= 0;) {
                output.write(buffer, 0, read);
            }
        }
    }

    private static void writeText(File file, String text) throws IOException {
        try (OutputStream output = new FileOutputStream(file)) {
            output.write(text.getBytes(java.nio.charset.StandardCharsets.UTF_8));
        }
    }

    private static void deleteTree(File file) {
        File[] children = file.listFiles();
        if (children != null) {
            for (File child : children) {
                deleteTree(child);
            }
        }
        if (file.exists()) {
            file.delete();
        }
    }
}
