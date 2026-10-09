# Saves a screenshot of every open Explorer folder window: shot.ps1 <out-prefix>
param([string]$Out)
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
using System.Runtime.InteropServices;
using System.Text;
public static class Shot {
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [StructLayout(LayoutKind.Sequential)] struct RECT { public int l, t, r, b; }
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    public static string Run(string prefix) {
        var sb = new StringBuilder(); int n = 0;
        EnumWindows((h, l) => {
            var c = new StringBuilder(64); GetClassName(h, c, 64);
            if (c.ToString() != "CabinetWClass" || !IsWindowVisible(h)) return true;
            var t = new StringBuilder(256); GetWindowText(h, t, 256);
            RECT r; GetWindowRect(h, out r);
            int w = r.r - r.l, ht = r.b - r.t;
            if (w < 50 || ht < 50) return true;
            using (var bmp = new Bitmap(w, ht)) using (var g = Graphics.FromImage(bmp)) {
                IntPtr hdc = g.GetHdc(); PrintWindow(h, hdc, 2); g.ReleaseHdc(hdc);
                string f = prefix + (n++) + ".png"; bmp.Save(f);
                sb.AppendLine(f + "  " + w + "x" + ht + "  " + t);
            }
            return true;
        }, IntPtr.Zero);
        return sb.ToString();
    }
}
'@
[Shot]::Run($Out)
