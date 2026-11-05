package dev.tslib;

/**
 * Opus encoder/decoder pair.
 *
 * <p>Not thread-safe.
 *
 * <pre>{@code
 * try (OpusCodec codec = new OpusCodec()) {
 *     byte[] encoded = codec.encode(pcmBytes);
 *     byte[] decoded = codec.decode(encoded);
 * }
 * }</pre>
 */
public class OpusCodec implements AutoCloseable {
    private long nativePtr;

    /**
     * Create an Opus codec with default configuration.
     *
     * @throws TsLibException if creation fails
     */
    public OpusCodec() {
        this(new AudioConfig());
    }

    /**
     * Create an Opus codec with the given configuration.
     *
     * @param config audio configuration
     * @throws TsLibException if creation fails
     */
    public OpusCodec(AudioConfig config) {
        this.nativePtr = nativeCreate(config.sampleRate, config.channels,
                                       config.bitrate, config.frameSizeMs);
        if (this.nativePtr == 0) {
            throw new TsLibException("Failed to create OpusCodec");
        }
    }

    /**
     * Encode PCM samples (16-bit little-endian bytes) to Opus.
     *
     * @param pcm raw PCM data
     * @return Opus-encoded packet
     * @throws TsLibException on encoding error
     */
    public byte[] encode(byte[] pcm) {
        checkNotClosed();
        return nativeEncode(nativePtr, pcm);
    }

    /**
     * Decode an Opus packet to PCM (16-bit little-endian bytes).
     *
     * @param data Opus-encoded data
     * @return decoded PCM samples
     * @throws TsLibException on decoding error
     */
    public byte[] decode(byte[] data) {
        return decode(data, false);
    }

    /**
     * Decode an Opus packet to PCM, optionally using FEC decoding. With
     * {@code fec = true} this returns the loss-repair audio reconstructed
     * from the FEC data embedded in this packet — call it on the frame
     * FOLLOWING a lost one to conceal the loss (the redundancy for frame N
     * travels inside frame N+1).
     *
     * @param data Opus-encoded data
     * @param fec  true to decode the embedded FEC repair data instead of the
     *             packet's own audio
     * @return decoded PCM samples
     * @throws TsLibException on decoding error
     */
    public byte[] decode(byte[] data, boolean fec) {
        checkNotClosed();
        return nativeDecode(nativePtr, data, fec);
    }

    /**
     * Conceal a lost packet using Opus PLC.
     *
     * @return PLC samples as 16-bit little-endian bytes
     * @throws TsLibException on decoding error
     */
    public byte[] decodePlc() {
        checkNotClosed();
        return nativeDecodePlc(nativePtr);
    }

    /**
     * Change the encoder bitrate at runtime.
     *
     * @param bitrate bits per second
     * @throws TsLibException on error
     */
    public void setBitrate(int bitrate) {
        checkNotClosed();
        nativeSetBitrate(nativePtr, bitrate);
    }

    /**
     * Toggle inband forward error correction.
     *
     * @param enabled true to enable FEC
     * @throws TsLibException on error
     */
    public void setFec(boolean enabled) {
        checkNotClosed();
        nativeSetFec(nativePtr, enabled);
    }

    /**
     * Set the packet loss percentage the encoder should optimize FEC for.
     *
     * @param percent expected loss percentage
     * @throws TsLibException on error
     */
    public void setExpectedPacketLoss(int percent) {
        checkNotClosed();
        nativeSetExpectedPacketLoss(nativePtr, percent);
    }

    /**
     * Get the audio configuration used by this codec.
     */
    public AudioConfig getConfig() {
        checkNotClosed();
        return nativeGetConfig(nativePtr);
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
            throw new IllegalStateException("OpusCodec has been closed");
        }
    }

    // Native methods
    private static native long nativeCreate(int sampleRate, int channels,
                                             int bitrate, int frameSizeMs);
    private static native void nativeDestroy(long ptr);
    private static native byte[] nativeEncode(long ptr, byte[] pcm);
    private static native byte[] nativeDecode(long ptr, byte[] data, boolean fec);
    private static native byte[] nativeDecodePlc(long ptr);
    private static native void nativeSetBitrate(long ptr, int bitrate);
    private static native void nativeSetFec(long ptr, boolean enabled);
    private static native void nativeSetExpectedPacketLoss(long ptr, int percent);
    private static native AudioConfig nativeGetConfig(long ptr);
}
