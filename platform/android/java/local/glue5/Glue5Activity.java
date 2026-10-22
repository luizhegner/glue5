package local.glue5;

import android.content.ContentResolver;
import android.content.Intent;
import android.content.SharedPreferences;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.system.ErrnoException;
import android.system.Os;
import android.provider.DocumentsContract;

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
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicBoolean;

/** The app's activity: SDL's, with our library (libglue5.so) as the main one. */
public class Glue5Activity extends SDLActivity {
    private static final int IMPORT_TREE = 4105;
    private static final String IMPORT_PREFS = "glue5-import";
    private static final String PENDING_KIND = "kind";
    private static final String PENDING_URI = "uri";
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
    public static void startImport(String kind) {
        Glue5Activity activity = active;
        if (activity == null || (!kind.equals("save") && !kind.equals("resources"))) {
            return;
        }
        activity.runOnUiThread(() -> activity.openImportTree(kind));
    }

    private void openImportTree(String kind) {
        SharedPreferences prefs = getSharedPreferences(IMPORT_PREFS, MODE_PRIVATE);
        prefs.edit().putString(PENDING_KIND, kind).remove(PENDING_URI).apply();
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION);
        startActivityForResult(intent, IMPORT_TREE);
    }

    @Override
    @SuppressWarnings("deprecation")
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
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
        File external = getExternalFilesDir(null);
        if (external == null) {
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
            writeText(new File(partial, ".glue5-source-name"), displayName(tree));
            copyTree(tree, partial);
            File ready = new File(new File(root, "ready"), token);
            if (!ready.getParentFile().mkdirs() && !ready.getParentFile().isDirectory()) {
                throw new IOException("cannot create completed imports folder");
            }
            if (!partial.renameTo(ready)) {
                throw new IOException("cannot finalize imported folder");
            }
            getSharedPreferences(IMPORT_PREFS, MODE_PRIVATE).edit().clear().apply();
        } catch (Exception error) {
            try {
                File errors = new File(root, "errors");
                errors.mkdirs();
                writeText(new File(errors, token + ".txt"), error.toString());
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
        try (Cursor cursor = getContentResolver().query(tree,
                new String[] { DocumentsContract.Document.COLUMN_DISPLAY_NAME }, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                return safeName(cursor.getString(0));
            }
        } catch (Exception ignored) {
        }
        return "imported-profile";
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
