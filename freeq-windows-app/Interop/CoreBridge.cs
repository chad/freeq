using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;

namespace Freeq.Windows.Interop;

/// <summary>
/// High-level wrapper around the Rust FFI. Manages the client handle lifecycle
/// and dispatches deserialized events to a callback on the caller's thread context.
/// </summary>
public sealed class CoreBridge : IDisposable
{
    private ulong _handle;
    private NativeMethods.EventCallback? _pinned; // prevent GC of delegate
    private bool _disposed;

    public event Action<EventEnvelope>? EventReceived;

    public ulong Handle => _handle;
    public bool IsCreated => _handle != 0;

    public bool Create(string server, string nick, bool tls)
    {
        var config = JsonSerializer.Serialize(new { server, nick, tls });
        _handle = NativeMethods.CreateClient(config);
        return _handle != 0;
    }

    public void SubscribeEvents()
    {
        if (_handle == 0) return;

        _pinned = OnNativeEvent;
        var result = NativeMethods.SubscribeEvents(_handle, _pinned, IntPtr.Zero);
        if (result != 0)
            throw new InvalidOperationException($"SubscribeEvents failed: {result}");
    }

    public void SetWebToken(string token)
    {
        if (_handle == 0) return;
        NativeMethods.SetWebToken(_handle, token);
    }

    public int Connect()
    {
        if (_handle == 0) return 1;
        return NativeMethods.Connect(_handle);
    }

    public int Disconnect()
    {
        if (_handle == 0) return 1;
        return NativeMethods.Disconnect(_handle);
    }

    public int Join(string channel)
    {
        if (_handle == 0) return 1;
        return NativeMethods.Join(_handle, channel);
    }

    public int SendMessage(string target, string text)
    {
        if (_handle == 0) return 1;
        return NativeMethods.SendMessage(_handle, target, text);
    }

    public int SendRaw(string line)
    {
        if (_handle == 0) return 1;
        return NativeMethods.SendRaw(_handle, line);
    }

    public string? GetSnapshotJson()
    {
        if (_handle == 0) return null;
        var ptr = NativeMethods.GetSnapshotJson(_handle);
        if (ptr == IntPtr.Zero) return null;
        try
        {
            return Marshal.PtrToStringUTF8(ptr);
        }
        finally
        {
            NativeMethods.FreeString(ptr);
        }
    }

    // ── Rich messaging ──

    public int Reply(string target, string msgid, string text)
    {
        if (_handle == 0) return 1;
        return NativeMethods.Reply(_handle, target, msgid, text);
    }

    public int EditMessage(string target, string msgid, string text)
    {
        if (_handle == 0) return 1;
        return NativeMethods.EditMessage(_handle, target, msgid, text);
    }

    public int DeleteMessage(string target, string msgid)
    {
        if (_handle == 0) return 1;
        return NativeMethods.DeleteMessage(_handle, target, msgid);
    }

    public int React(string target, string emoji, string msgid)
    {
        if (_handle == 0) return 1;
        return NativeMethods.React(_handle, target, emoji, msgid);
    }

    public int TypingStart(string target)
    {
        if (_handle == 0) return 1;
        return NativeMethods.TypingStart(_handle, target);
    }

    public int TypingStop(string target)
    {
        if (_handle == 0) return 1;
        return NativeMethods.TypingStop(_handle, target);
    }

    public int HistoryLatest(string target, uint count)
    {
        if (_handle == 0) return 1;
        return NativeMethods.HistoryLatest(_handle, target, count);
    }

    public int HistoryBefore(string target, string msgid, uint count)
    {
        if (_handle == 0) return 1;
        return NativeMethods.HistoryBefore(_handle, target, msgid, count);
    }

    public int Pin(string channel, string msgid)
    {
        if (_handle == 0) return 1;
        return NativeMethods.Pin(_handle, channel, msgid);
    }

    public int Unpin(string channel, string msgid)
    {
        if (_handle == 0) return 1;
        return NativeMethods.Unpin(_handle, channel, msgid);
    }

    public int SendTagged(string target, string text, string tagsJson)
    {
        if (_handle == 0) return 1;
        return NativeMethods.SendTagged(_handle, target, text, tagsJson);
    }

    public int SendTagmsg(string target, string tagsJson)
    {
        if (_handle == 0) return 1;
        return NativeMethods.SendTagmsg(_handle, target, tagsJson);
    }

    public int Mode(string channel, string flags, string? arg)
    {
        if (_handle == 0) return 1;
        return NativeMethods.Mode(_handle, channel, flags, arg);
    }

    /// <summary>
    /// Native callback invoked from the Rust event pump thread.
    /// Deserializes the JSON envelope and raises EventReceived.
    /// </summary>
    private void OnNativeEvent(IntPtr jsonPtr, nuint jsonLen, IntPtr userData)
    {
        try
        {
            var json = Marshal.PtrToStringUTF8(jsonPtr, (int)jsonLen);
            if (json == null) return;

            var envelope = JsonSerializer.Deserialize<EventEnvelope>(json);
            if (envelope != null)
            {
                EventReceived?.Invoke(envelope);
            }
        }
        catch
        {
            // Never let exceptions propagate back into Rust
        }
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;

        if (_handle != 0)
        {
            NativeMethods.DestroyClient(_handle);
            _handle = 0;
        }
        _pinned = null;
    }
}
