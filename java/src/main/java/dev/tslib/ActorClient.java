package dev.tslib;

/**
 * An actor-backed TeamSpeak client.
 *
 * <p>The connection runs on a dedicated native thread. Commands block until the
 * actor hands them off (with a timeout) and report the real result, while
 * {@link #pumpEvents(int)} drains queued events without blocking the network.
 *
 * <p>This class is an alternative to {@link Client}; the legacy client remains
 * available for compatibility.
 */
public class ActorClient implements AutoCloseable {
    private long nativePtr;

    /**
     * Connect and start the actor thread.
     *
     * @param address  server address (host:port)
     * @param identity identity to authenticate with
     * @param nickname display name
     * @param password server password, may be null
     * @throws TsLibException if the connection cannot be established
     */
    public ActorClient(String address, Identity identity, String nickname, String password) {
        if (address == null || identity == null || nickname == null) {
            throw new TsLibException("address, identity and nickname must not be null");
        }
        this.nativePtr = nativeCreate(address, identity.nativePtr(), nickname, password);
        if (this.nativePtr == 0) {
            throw new TsLibException("Failed to connect actor client");
        }
    }

    private void checkOpen() {
        if (nativePtr == 0) {
            throw new IllegalStateException("ActorClient is closed");
        }
    }

    /**
     * Drain pending events, waiting up to {@code timeoutMs} for the first one.
     *
     * @return events (never null)
     */
    public Event[] pumpEvents(int timeoutMs) {
        checkOpen();
        return nativePump(nativePtr, timeoutMs);
    }

    /** Send a server-wide message and wait for the actor result. */
    public int sendServerMessage(String message, int timeoutMs) {
        checkOpen();
        return nativeSendServerMessage(nativePtr, message, timeoutMs);
    }

    /** Send a private message and wait for the actor result. */
    public int sendPrivateMessage(int userId, String message, int timeoutMs) {
        checkOpen();
        return nativeSendPrivateMessage(nativePtr, userId, message, timeoutMs);
    }

    /** Move to a channel and wait for the actor result. */
    public int moveToChannel(long channelId, int timeoutMs) {
        checkOpen();
        return nativeMoveToChannel(nativePtr, channelId, timeoutMs);
    }

    /** Update our input mute state and wait for the actor result. */
    public int setInputMuted(boolean muted, int timeoutMs) {
        checkOpen();
        return nativeSetInputMuted(nativePtr, muted, timeoutMs);
    }

    /** Current connection state (0=disconnected, 1=connecting, 2=connected, 3=reconnecting). */
    public int state() {
        return nativePtr == 0 ? 0 : nativeState(nativePtr);
    }

    /** Whether the actor is connected. */
    public boolean isConnected() {
        return state() == 2;
    }

    /** Disconnect and stop the actor thread. */
    public void disconnect() {
        if (nativePtr != 0) {
            nativeDisconnect(nativePtr);
        }
    }

    @Override
    public void close() {
        if (nativePtr != 0) {
            nativeFree(nativePtr);
            nativePtr = 0;
        }
    }

    private static native long nativeCreate(String address, long identityPtr, String nickname, String password);
    private static native Event[] nativePump(long ptr, int timeoutMs);
    private static native int nativeSendServerMessage(long ptr, String message, int timeoutMs);
    private static native int nativeSendPrivateMessage(long ptr, int userId, String message, int timeoutMs);
    private static native int nativeMoveToChannel(long ptr, long channelId, int timeoutMs);
    private static native int nativeSetInputMuted(long ptr, boolean muted, int timeoutMs);
    private static native int nativeState(long ptr);
    private static native void nativeDisconnect(long ptr);
    private static native void nativeFree(long ptr);
}
