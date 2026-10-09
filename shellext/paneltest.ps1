# Hosts the preview panel in a throwaway window and saves a screenshot: paneltest.ps1 <flp> <out.png>
param([string]$Path, [string]$Out, [string]$Search = '')
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
using System.Runtime.InteropServices;
using System.Windows.Forms;
public static class PanelTest {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int l, t, r, b; }
    [ComImport, Guid("8895b1c6-b41f-4c1c-a562-0d564250836f"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IPreviewHandler {
        [PreserveSig] int SetWindow(IntPtr hwnd, ref RECT rc);
        [PreserveSig] int SetRect(ref RECT rc);
        [PreserveSig] int DoPreview();
        [PreserveSig] int Unload();
        [PreserveSig] int SetFocus();
        [PreserveSig] int QueryFocus(out IntPtr hwnd);
        [PreserveSig] int TranslateAccelerator(IntPtr msg);
    }
    [ComImport, Guid("B7D14566-0509-4CCE-A71F-0A554233BD9B"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IInitializeWithFile { [PreserveSig] int Initialize([MarshalAs(UnmanagedType.LPWStr)] string path, uint mode); }
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr FindWindowEx(IntPtr parent, IntPtr after, string cls, string title);
    [DllImport("user32.dll")] static extern IntPtr GetDlgItem(IntPtr dlg, int id);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, string l);
    public static string Run(string path, string outPng, string search) {
        // first a throwaway preview, like Explorer selecting one file and then another
        {
            object o0 = Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("3E9A1D52-8C47-4B6F-A1E3-5D7F2B9C6A21")));
            ((IInitializeWithFile)o0).Initialize(path, 0);
            var f0 = new Form { ClientSize = new Size(300, 300) };
            f0.Show();
            var r0 = new RECT { l = 0, t = 0, r = 300, b = 300 };
            ((IPreviewHandler)o0).SetWindow(f0.Handle, ref r0);
            ((IPreviewHandler)o0).DoPreview();
            Application.DoEvents();
            ((IPreviewHandler)o0).Unload();
            Marshal.FinalReleaseComObject(o0);
            f0.Close();
            Application.DoEvents();
        }
        object o = Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("3E9A1D52-8C47-4B6F-A1E3-5D7F2B9C6A21")));
        int hi = ((IInitializeWithFile)o).Initialize(path, 0);
        var f = new Form { Text = "FL Library panel test", ClientSize = new Size(520, 760), StartPosition = FormStartPosition.Manual, Location = new Point(60, 40), TopMost = true };
        f.Show();
        var ph = (IPreviewHandler)o;
        var rc = new RECT { l = 0, t = 0, r = f.ClientSize.Width, b = f.ClientSize.Height };
        int hs = ph.SetWindow(f.Handle, ref rc);
        int hd = ph.DoPreview();
        if (search != "") {
            IntPtr panel = FindWindowEx(f.Handle, IntPtr.Zero, "FLLibraryPanel", null);
            SendMessage(GetDlgItem(panel, 161), 0x000C, IntPtr.Zero, search);
        }
        for (int i = 0; i < 30; i++) { Application.DoEvents(); System.Threading.Thread.Sleep(50); }
        using (var bmp = new Bitmap(f.Width, f.Height))
        using (var g = Graphics.FromImage(bmp)) {
            IntPtr hdc = g.GetHdc();
            PrintWindow(f.Handle, hdc, 2);
            g.ReleaseHdc(hdc);
            bmp.Save(outPng);
        }
        int hu = ph.Unload();
        f.Close();
        return String.Format("Initialize=0x{0:X8} SetWindow=0x{1:X8} DoPreview=0x{2:X8} Unload=0x{3:X8}", hi, hs, hd, hu);
    }
}
'@
[PanelTest]::Run($Path, $Out, $Search)
