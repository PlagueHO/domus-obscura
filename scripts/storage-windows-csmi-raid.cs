// Offline candidate, not an approved command sender or production identity.
// Wire-layout reference: Intel QST SDK CSMIIOCTL.h, BSD-3-Clause,
// copyright Intel Corporation 2005-2009, mirrored at gigaplex/Intel_QST_SDK
// commit db5dce39a1b2e245bd5e3becc67585e05f4456a6.
// Only protocol offsets and sizes are used; no vendor implementation is copied.
using System;
using System.Collections.Generic;
using System.IO;
using System.Text;

public static class StorageWindowsCsmiRaid
{
    private static string Field(byte[] bytes, int offset, int width, bool required)
    {
        int end = offset + width;
        while (end > offset && (bytes[end - 1] == 0 || bytes[end - 1] == 32)) end--;
        if (required && end == offset)
            throw new IOException("Missing required candidate member identity field.");
        for (int i = offset; i < end; i++)
            if (bytes[i] < 32 || bytes[i] > 126)
                throw new IOException("Unsupported candidate member field encoding.");
        return Encoding.ASCII.GetString(bytes, offset, end - offset);
    }

    public static int Decode(byte[] bytes, uint used, uint expectedSet, int capacity,
        ushort major, ushort minor)
    {
        if (major != 0 || minor != 81)
            throw new NotSupportedException("The historical candidate is scoped only to observed CSMI 0.81.");
        if (capacity < 1 || capacity > 6 || bytes == null || used < 192 ||
            used > bytes.Length || used > 64 + capacity * 128 || (used - 64) % 128 != 0)
            throw new IOException("Invalid bounded historical RAID response.");
        if (BitConverter.ToUInt32(bytes, 0) != 28 ||
            Encoding.ASCII.GetString(bytes, 4, 8) != "CSMIARY\0" ||
            BitConverter.ToUInt32(bytes, 16) != 11 ||
            BitConverter.ToUInt32(bytes, 20) != 0 ||
            BitConverter.ToUInt32(bytes, 24) != used - 28 ||
            BitConverter.ToUInt32(bytes, 28) != expectedSet)
            throw new IOException("Historical RAID response header or set does not match.");
        int count = bytes[43];
        if (count < 1 || count > capacity || used < 64 + count * 128)
            throw new IOException("Historical RAID member count exceeds the response.");
        for (int i = 44; i < 64; i++)
            if (bytes[i] != 0)
                throw new NotSupportedException("Historical RAID prefix extension is unsupported.");
        var identities = new HashSet<string>(StringComparer.Ordinal);
        for (int member = 0; member < count; member++)
        {
            int position = 64 + member * 128;
            string model = Field(bytes, position, 40, true);
            Field(bytes, position + 40, 8, false);
            string serial = Field(bytes, position + 48, 40, true);
            if (!identities.Add(model + "\0" + serial))
                throw new IOException("Duplicate candidate model and serial tuple.");
            for (int i = position + 98; i < position + 128; i++)
                if (bytes[i] != 0)
                    throw new NotSupportedException("Historical RAID member extension is unsupported.");
        }
        for (int i = 64 + count * 128; i < used; i++)
            if (bytes[i] != 0)
                throw new IOException("Undeclared historical RAID member data.");
        return count;
    }
}
