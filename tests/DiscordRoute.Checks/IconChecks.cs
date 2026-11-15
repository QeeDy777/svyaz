using System.Buffers.Binary;

internal static class IconChecks
{
    public static void Run(Action<bool, string> check)
    {
        var bytes = File.ReadAllBytes(Path.Combine(AppContext.BaseDirectory, "app.ico"));
        check(IsCompatible(bytes), "App ICO contains seven valid 32-bit RGBA PNG frames for Windows");
        var rgb = (byte[])bytes.Clone();
        var firstFrame = BinaryPrimitives.ReadInt32LittleEndian(rgb.AsSpan(18));
        rgb[firstFrame + 25] = 2;
        check(!IsCompatible(rgb), "Reject 24-bit RGB PNG advertised as 32-bit ICO (startup regression)");
        check(!IsCompatible(bytes[..30]), "Reject incomplete icon directory");
    }

    private static bool IsCompatible(ReadOnlySpan<byte> bytes)
    {
        int[] sizes = [16, 24, 32, 48, 64, 128, 256];
        if (bytes.Length < 6 + 16 * sizes.Length ||
            BinaryPrimitives.ReadUInt16LittleEndian(bytes) != 0 ||
            BinaryPrimitives.ReadUInt16LittleEndian(bytes[2..]) != 1 ||
            BinaryPrimitives.ReadUInt16LittleEndian(bytes[4..]) != sizes.Length) return false;
        for (var index = 0; index < sizes.Length; index++)
        {
            var entry = bytes.Slice(6 + 16 * index, 16);
            var size = BinaryPrimitives.ReadInt32LittleEndian(entry[8..]);
            var offset = BinaryPrimitives.ReadInt32LittleEndian(entry[12..]);
            if ((entry[0] == 0 ? 256 : entry[0]) != sizes[index] || entry[0] != entry[1] ||
                BinaryPrimitives.ReadUInt16LittleEndian(entry[4..]) != 1 ||
                BinaryPrimitives.ReadUInt16LittleEndian(entry[6..]) != 32 ||
                offset < 6 + 16 * sizes.Length || size < 33 || offset > bytes.Length - size) return false;
            var png = bytes.Slice(offset, size);
            if (!png[..8].SequenceEqual(new byte[] { 137, 80, 78, 71, 13, 10, 26, 10 }) ||
                !png.Slice(12, 4).SequenceEqual("IHDR"u8) ||
                BinaryPrimitives.ReadInt32BigEndian(png[16..]) != sizes[index] ||
                BinaryPrimitives.ReadInt32BigEndian(png[20..]) != sizes[index] ||
                png[24] != 8 || png[25] != 6) return false;
        }
        return true;
    }
}
