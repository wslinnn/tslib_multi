package dev.tslib;

/**
 * Acoustic echo canceller (WebRTC AEC3).
 *
 * <p>All frames are 16-bit PCM. {@code mic}, {@code far} and {@code out} must
 * each have exactly {@code frameSamples} samples.</p>
 *
 * <p>Not thread-safe. Call {@link #close()} when done.</p>
 */
public class EchoCanceller implements AutoCloseable {
    private long nativePtr;

    /**
     * Create an echo canceller.
     *
     * @param sampleRate   sample rate in Hz (16 kHz, 32 kHz or 48 kHz)
     * @param frameSamples samples per frame; must be a multiple of 10 ms
     * @param tailSamples  reference tail length in samples (used by the AEC
     *                     configuration; must be positive)
     * @throws TsLibException if creation fails
     */
    public EchoCanceller(int sampleRate, int frameSamples, int tailSamples) {
        this.nativePtr = nativeCreate(sampleRate, frameSamples, tailSamples);
        if (this.nativePtr == 0) {
            throw new TsLibException("Failed to create EchoCanceller");
        }
    }

    /**
     * Cancel echo in one frame.
     *
     * @param mic near-end microphone samples
     * @param far far-end reference samples (what the speaker played)
     * @param out output buffer, filled with the cleaned samples
     * @return number of samples written, or -1 on error
     * @throws TsLibException on error
     */
    public int process(short[] mic, short[] far, short[] out) {
        checkNotClosed();
        return nativeProcess(nativePtr, mic, far, out);
    }

    /**
     * Discard the learned echo path, e.g. after a route change.
     */
    public void reset() {
        checkNotClosed();
        nativeReset(nativePtr);
    }

    /**
     * Number of frames zeroed by the output watchdog since creation.
     */
    public long getZeroedFrames() {
        checkNotClosed();
        return nativeZeroedFrames(nativePtr);
    }

    @Override
    public void close() {
        if (nativePtr != 0) {
            nativeDestroy(nativePtr);
            nativePtr = 0;
        }
    }

    private void checkNotClosed() {
        if (nativePtr == 0) {
            throw new IllegalStateException("EchoCanceller has been closed");
        }
    }

    // Native methods
    private static native long nativeCreate(int sampleRate, int frameSamples, int tailSamples);
    private static native void nativeDestroy(long ptr);
    private static native void nativeReset(long ptr);
    private static native long nativeZeroedFrames(long ptr);
    private static native int nativeProcess(long ptr, short[] mic, short[] far, short[] out);
}
