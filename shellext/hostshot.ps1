# Reports the companion panel window's state and saves a picture of it: hostshot.ps1 <out.png>
param([string]$Out)
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
using System.Runtime.InteropServices;
public static class HostShot {
    [StructLayout(LayoutKind.Sequential)] struct RECT { public int l, t, r, b; }
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr FindWindow(string c, IntPtr t);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] static extern bool SetProcessDpiAwarenessContext(IntPtr v);
    public static string Run(string outPng) {
        SetProcessDpiAwarenessContext(new IntPtr(-4));
        IntPtr h = FindWindow("FLLibraryHost", IntPtr.Zero);
        if (h == IntPtr.Zero) return "host window not found";
        RECT r; GetWindowRect(h, out r);
        int w = r.r - r.l, ht = r.b - r.t;
        string s = "host=" + h + " visible=" + IsWindowVisible(h) + " rect=" + r.l + "," + r.t + " " + w + "x" + ht;
        if (w > 10 && ht > 10) {
            using (var bmp = new Bitmap(w, ht)) using (var g = Graphics.FromImage(bmp)) {
                IntPtr hdc = g.GetHdc(); PrintWindow(h, hdc, 2); g.ReleaseHdc(hdc);
                bmp.Save(outPng);
            }
        }
        return s;
    }
}
'@
[HostShot]::Run($Out)
