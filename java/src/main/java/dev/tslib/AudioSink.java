package dev.tslib;

/**
 * Low-latency callback for received voice frames.
 *
 * <p>Called on the thread that pumps the client. Implementations must return
 * quickly and must not call back into the same {@link Client}, otherwise the
 * connection loop can stall or deadlock.</p>
 */
public interface AudioSink {
    /**
     * @param userId    sender client ID
     * @param seq       sender's voice packet sequence (wraps at 65536)
     * @param data      encoded voice frame (may be empty for end-of-speech)
     * @param isWhisper whether the frame was a whisper
     */
    void onAudioFrame(int userId, int seq, byte[] data, boolean isWhisper);
}
