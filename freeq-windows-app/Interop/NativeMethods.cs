using System.Runtime.InteropServices;

namespace Freeq.Windows.Interop;

/// <summary>
/// P/Invoke declarations for freeq_windows_core.dll (Rust C ABI).
/// </summary>
internal static partial class NativeMethods
{
    private const string DllName = "freeq_windows_core";

    /// <summary>
    /// Callback signature matching Rust: extern "C" fn(json_ptr, json_len, user_data).
    /// </summary>
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    public delegate void EventCallback(IntPtr jsonPtr, nuint jsonLen, IntPtr userData);

    [LibraryImport(DllName, EntryPoint = "freeq_win_create_client", StringMarshalling = StringMarshalling.Utf8)]
    public static partial ulong CreateClient(string configJson);

    [LibraryImport(DllName, EntryPoint = "freeq_win_destroy_client")]
    public static partial void DestroyClient(ulong handle);

    [LibraryImport(DllName, EntryPoint = "freeq_win_subscribe_events")]
    public static partial int SubscribeEvents(ulong handle, EventCallback cb, IntPtr userData);

    [LibraryImport(DllName, EntryPoint = "freeq_win_set_web_token", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int SetWebToken(ulong handle, string token);

    [LibraryImport(DllName, EntryPoint = "freeq_win_connect")]
    public static partial int Connect(ulong handle);

    [LibraryImport(DllName, EntryPoint = "freeq_win_disconnect")]
    public static partial int Disconnect(ulong handle);

    [LibraryImport(DllName, EntryPoint = "freeq_win_join", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int Join(ulong handle, string channel);

    [LibraryImport(DllName, EntryPoint = "freeq_win_send_message", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int SendMessage(ulong handle, string target, string text);

    [LibraryImport(DllName, EntryPoint = "freeq_win_send_raw", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int SendRaw(ulong handle, string line);

    [LibraryImport(DllName, EntryPoint = "freeq_win_get_snapshot_json")]
    public static partial IntPtr GetSnapshotJson(ulong handle);

    [LibraryImport(DllName, EntryPoint = "freeq_win_free_string")]
    public static partial void FreeString(IntPtr ptr);

    // ── Rich messaging ──

    [LibraryImport(DllName, EntryPoint = "freeq_win_reply", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int Reply(ulong handle, string target, string msgid, string text);

    [LibraryImport(DllName, EntryPoint = "freeq_win_edit_message", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int EditMessage(ulong handle, string target, string msgid, string text);

    [LibraryImport(DllName, EntryPoint = "freeq_win_delete_message", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int DeleteMessage(ulong handle, string target, string msgid);

    [LibraryImport(DllName, EntryPoint = "freeq_win_react", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int React(ulong handle, string target, string emoji, string msgid);

    [LibraryImport(DllName, EntryPoint = "freeq_win_typing_start", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int TypingStart(ulong handle, string target);

    [LibraryImport(DllName, EntryPoint = "freeq_win_typing_stop", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int TypingStop(ulong handle, string target);

    [LibraryImport(DllName, EntryPoint = "freeq_win_history_latest", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int HistoryLatest(ulong handle, string target, uint count);

    [LibraryImport(DllName, EntryPoint = "freeq_win_history_before", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int HistoryBefore(ulong handle, string target, string msgid, uint count);

    [LibraryImport(DllName, EntryPoint = "freeq_win_pin", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int Pin(ulong handle, string channel, string msgid);

    [LibraryImport(DllName, EntryPoint = "freeq_win_unpin", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int Unpin(ulong handle, string channel, string msgid);

    [LibraryImport(DllName, EntryPoint = "freeq_win_send_tagged", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int SendTagged(ulong handle, string target, string text, string tagsJson);

    [LibraryImport(DllName, EntryPoint = "freeq_win_send_tagmsg", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int SendTagmsg(ulong handle, string target, string tagsJson);

    [LibraryImport(DllName, EntryPoint = "freeq_win_mode", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int Mode(ulong handle, string channel, string flags, string? arg);
}
