using System;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.IO.Compression;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using System.Windows.Forms;

internal static class KynnVoiceSetup
{
    private const string AppResourceName = "KynnVoiceApp";
    private const string DriverResourceName = "VBCableZip";
    private const string LicenseResourceName = "KynnVoiceLicense";
    private const string NoticeResourceName = "VBCableNotice";
    private const string ThirdPartyNoticesResourceName = "KynnVoiceThirdPartyNotices";
    private const string ThirdPartyLicensesResourceName = "KynnVoiceThirdPartyLicenses";
    private const string AppFileName = "kynn-voice.exe";
    private const string DriverZipFileName = "VBCABLE_Driver_Pack45.zip";
    private const string DriverAudioWarning = "A propria VB-Audio informa que, no Windows 10/11, o ultimo driver de audio instalado pode ser escolhido automaticamente como dispositivo padrao. Se CABLE Input virar sua saida padrao, o audio do PC vai para o cabo e voce pode deixar de ouvir pelos alto-falantes ou fones. Depois da instalacao e da reinicializacao, confira Configuracoes > Sistema > Som > Saida e selecione manualmente seus alto-falantes ou fones. No Discord ou jogo, use CABLE Output como microfone. O Kynn Voice nao altera esses dispositivos automaticamente.";

    private const string ExpectedAppSha256 = "__APP_SHA256__";
    private const string ExpectedDriverZipSha256 = "__DRIVER_ZIP_SHA256__";
    private const string ExpectedDriverSetupSha256 = "__DRIVER_SETUP_SHA256__";
    private const string ExpectedDriverCatalogSha256 = "__DRIVER_CATALOG_SHA256__";
    private const string ExpectedThirdPartyNoticesSha256 = "__THIRD_PARTY_NOTICES_SHA256__";
    private const string ExpectedThirdPartyLicensesSha256 = "__THIRD_PARTY_LICENSES_SHA256__";

    [STAThread]
    private static int Main(string[] args)
    {
        try
        {
            Payload payload = Payload.Load();
            VerifyPayload(payload);

            if (args.Length == 1 && String.Equals(args[0], "--verify-only", StringComparison.OrdinalIgnoreCase))
            {
                return 0;
            }

            if (!Environment.Is64BitOperatingSystem)
            {
                MessageBox.Show("Este pacote requer Windows de 64 bits.", "Kynn Voice Setup", MessageBoxButtons.OK, MessageBoxIcon.Error);
                return 2;
            }

            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);

            DialogResult choice;
            using (SetupDialog dialog = new SetupDialog(payload.NoticeText))
            {
                choice = dialog.ShowDialog();
            }

            if (choice == DialogResult.Cancel || (choice != DialogResult.Yes && choice != DialogResult.No))
            {
                return 0;
            }

            if (choice == DialogResult.Yes)
            {
                DialogResult driverConsent = MessageBox.Show(
                    DriverAudioWarning + "\r\n\r\nDeseja continuar e iniciar o instalador oficial do VB-CABLE?",
                    "Antes de instalar o VB-CABLE",
                    MessageBoxButtons.YesNo,
                    MessageBoxIcon.Warning);
                if (driverConsent != DialogResult.Yes)
                {
                    return 0;
                }
            }

            string processName = Path.GetFileNameWithoutExtension(AppFileName);
            if (Process.GetProcessesByName(processName).Length > 0)
            {
                MessageBox.Show("Feche o Kynn Voice que ja esta aberto e execute este instalador novamente.", "Kynn Voice Setup", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                return 3;
            }

            string installDirectory = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "Programs",
                "Kynn Voice");
            Directory.CreateDirectory(installDirectory);

            string installedAppPath = Path.Combine(installDirectory, AppFileName);
            File.WriteAllBytes(installedAppPath, payload.AppBytes);
            File.WriteAllBytes(Path.Combine(installDirectory, "KynnVoice-LICENSE.txt"), payload.LicenseBytes);
            File.WriteAllBytes(Path.Combine(installDirectory, "VB-CABLE-NOTICE.txt"), payload.NoticeBytes);
            File.WriteAllBytes(Path.Combine(installDirectory, "THIRD_PARTY_NOTICES.md"), payload.ThirdPartyNoticesBytes);
            File.WriteAllBytes(Path.Combine(installDirectory, "THIRD_PARTY_LICENSES.txt"), payload.ThirdPartyLicensesBytes);
            CreateStartMenuShortcut(installedAppPath);

            if (choice == DialogResult.No)
            {
                MessageBox.Show(
                    "Kynn Voice foi instalado em:\r\n" + installDirectory + "\r\n\r\nO VB-CABLE nao foi instalado. Sem um dispositivo virtual, outros aplicativos nao podem receber a saida do Kynn Voice.",
                    "Kynn Voice Setup",
                    MessageBoxButtons.OK,
                    MessageBoxIcon.Information);
                return 0;
            }

            InstallDriver(payload.DriverZipBytes, installDirectory);
            return 0;
        }
        catch (Exception error)
        {
            MessageBox.Show(
                "O Setup do Kynn Voice encontrou um erro. Se o instalador do VB-CABLE chegou a ser iniciado, confira manualmente a saida e a entrada em Configuracoes > Sistema > Som.\r\n\r\n" + error.Message,
                "Kynn Voice Setup",
                MessageBoxButtons.OK,
                MessageBoxIcon.Error);
            return 1;
        }
    }

    private static void VerifyPayload(Payload payload)
    {
        AssertHash(payload.AppBytes, ExpectedAppSha256, "Kynn Voice executable");
        AssertHash(payload.DriverZipBytes, ExpectedDriverZipSha256, "VB-CABLE archive");
        AssertHash(payload.ThirdPartyNoticesBytes, ExpectedThirdPartyNoticesSha256, "Third-party notices");
        AssertHash(payload.ThirdPartyLicensesBytes, ExpectedThirdPartyLicensesSha256, "Third-party license texts");

        using (MemoryStream stream = new MemoryStream(payload.DriverZipBytes, false))
        using (ZipArchive archive = new ZipArchive(stream, ZipArchiveMode.Read, false))
        {
            bool foundSetup = false;
            bool foundCatalog = false;
            bool foundInf = false;
            bool foundDriver = false;

            foreach (ZipArchiveEntry entry in archive.Entries)
            {
                string name = entry.FullName;
                if (Path.IsPathRooted(name) || name.IndexOf("..", StringComparison.Ordinal) >= 0 || name.IndexOf(':') >= 0 || name.IndexOf('/') >= 0 || name.IndexOf('\\') >= 0)
                {
                    throw new InvalidDataException("O pacote VB-CABLE contem um caminho inesperado: " + name);
                }

                if (String.Equals(name, "VBCABLE_Setup_x64.exe", StringComparison.Ordinal))
                {
                    foundSetup = true;
                    AssertHash(ReadEntry(entry), ExpectedDriverSetupSha256, "VB-CABLE x64 setup");
                }
                else if (String.Equals(name, "vbaudio_cable64_win10.cat", StringComparison.Ordinal))
                {
                    foundCatalog = true;
                    AssertHash(ReadEntry(entry), ExpectedDriverCatalogSha256, "VB-CABLE Windows x64 catalog");
                }
                else if (String.Equals(name, "vbMmeCable64_win10.inf", StringComparison.Ordinal))
                {
                    foundInf = true;
                }
                else if (String.Equals(name, "vbaudio_cable64_win10.sys", StringComparison.Ordinal))
                {
                    foundDriver = true;
                }
            }

            if (!foundSetup || !foundCatalog || !foundInf || !foundDriver)
            {
                throw new InvalidDataException("O pacote VB-CABLE nao contem todos os arquivos esperados para Windows x64.");
            }
        }
    }

    private static byte[] ReadEntry(ZipArchiveEntry entry)
    {
        using (Stream input = entry.Open())
        using (MemoryStream output = new MemoryStream())
        {
            input.CopyTo(output);
            return output.ToArray();
        }
    }

    private static void AssertHash(byte[] data, string expected, string description)
    {
        string actual;
        using (SHA256 sha256 = SHA256.Create())
        {
            StringBuilder text = new StringBuilder();
            foreach (byte value in sha256.ComputeHash(data))
            {
                text.Append(value.ToString("x2"));
            }
            actual = text.ToString();
        }

        if (!String.Equals(actual, expected, StringComparison.OrdinalIgnoreCase))
        {
            throw new InvalidDataException(description + " SHA-256 nao confere. Esperado " + expected + "; recebido " + actual + ".");
        }
    }

    private static void InstallDriver(byte[] driverZipBytes, string installDirectory)
    {
        string tempDirectory = Path.Combine(Path.GetTempPath(), "KynnVoice-VBCABLE-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(tempDirectory);

        try
        {
            string zipPath = Path.Combine(tempDirectory, DriverZipFileName);
            File.WriteAllBytes(zipPath, driverZipBytes);
            ZipFile.ExtractToDirectory(zipPath, tempDirectory);

            string setupPath = Path.Combine(tempDirectory, "VBCABLE_Setup_x64.exe");
            string catalogPath = Path.Combine(tempDirectory, "vbaudio_cable64_win10.cat");
            AssertHash(File.ReadAllBytes(setupPath), ExpectedDriverSetupSha256, "VB-CABLE x64 setup extraido");
            AssertHash(File.ReadAllBytes(catalogPath), ExpectedDriverCatalogSha256, "VB-CABLE catalogo x64 extraido");

            ProcessStartInfo startInfo = new ProcessStartInfo();
            startInfo.FileName = setupPath;
            startInfo.WorkingDirectory = tempDirectory;
            startInfo.UseShellExecute = true;
            startInfo.Verb = "runas";

            using (Process driverProcess = Process.Start(startInfo))
            {
                if (driverProcess == null)
                {
                    throw new InvalidOperationException("O Windows nao iniciou o instalador oficial do VB-CABLE.");
                }
                driverProcess.WaitForExit();

                string message = "Kynn Voice foi instalado em:\r\n" + installDirectory + "\r\n\r\n" +
                    "O instalador oficial do VB-CABLE encerrou com codigo " + driverProcess.ExitCode + ". Confira a confirmacao exibida pelo instalador da VB-Audio. Se o driver foi instalado, reinicie o Windows antes de usa-lo, conforme orientacao do fabricante.\r\n\r\n" +
                    "Depois da reinicializacao, confira Configuracoes > Sistema > Som > Saida e selecione seus alto-falantes ou fones. O Windows pode ter escolhido o VB-CABLE como dispositivo padrao; se isso ocorrer, o audio do PC nao sera reproduzido nos alto-falantes ate que voce selecione a saida fisica. Para chamadas e jogos, escolha CABLE Output como microfone.";
                MessageBox.Show(message, "Kynn Voice Setup", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            }
        }
        catch (System.ComponentModel.Win32Exception error)
        {
            if (error.NativeErrorCode == 1223)
            {
                MessageBox.Show(
                    "Kynn Voice foi instalado. A solicitacao UAC foi cancelada, entao o VB-CABLE nao foi iniciado. Voce pode instalar o driver depois pelo site oficial: https://vb-cable.com/",
                    "Kynn Voice Setup",
                    MessageBoxButtons.OK,
                    MessageBoxIcon.Warning);
                return;
            }
            throw;
        }
        finally
        {
            DeleteTemporaryDirectory(tempDirectory);
        }
    }

    private static void DeleteTemporaryDirectory(string directory)
    {
        string tempRoot = Path.GetFullPath(Path.GetTempPath()).TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
        string fullPath = Path.GetFullPath(directory).TrimEnd(Path.DirectorySeparatorChar);
        string leaf = Path.GetFileName(fullPath);

        if (fullPath.StartsWith(tempRoot, StringComparison.OrdinalIgnoreCase) && leaf.StartsWith("KynnVoice-VBCABLE-", StringComparison.Ordinal))
        {
            Directory.Delete(fullPath, true);
        }
    }

    private static void CreateStartMenuShortcut(string executablePath)
    {
        string programs = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.StartMenu), "Programs");
        Directory.CreateDirectory(programs);
        string shortcutPath = Path.Combine(programs, "Kynn Voice.lnk");
        Type shellType = Type.GetTypeFromProgID("WScript.Shell");
        if (shellType == null)
        {
            throw new InvalidOperationException("O Windows Shell nao esta disponivel para criar o atalho do Kynn Voice.");
        }

        object shell = Activator.CreateInstance(shellType);
        object shortcut = null;
        try
        {
            shortcut = shellType.InvokeMember("CreateShortcut", BindingFlags.InvokeMethod, null, shell, new object[] { shortcutPath });
            Type shortcutType = shortcut.GetType();
            shortcutType.InvokeMember("TargetPath", BindingFlags.SetProperty, null, shortcut, new object[] { executablePath });
            shortcutType.InvokeMember("WorkingDirectory", BindingFlags.SetProperty, null, shortcut, new object[] { Path.GetDirectoryName(executablePath) });
            shortcutType.InvokeMember("IconLocation", BindingFlags.SetProperty, null, shortcut, new object[] { executablePath + ",0" });
            shortcutType.InvokeMember("Description", BindingFlags.SetProperty, null, shortcut, new object[] { "Processamento local do microfone com Kynn Voice" });
            shortcutType.InvokeMember("Save", BindingFlags.InvokeMethod, null, shortcut, null);
        }
        finally
        {
            if (shortcut != null && Marshal.IsComObject(shortcut)) Marshal.FinalReleaseComObject(shortcut);
            if (Marshal.IsComObject(shell)) Marshal.FinalReleaseComObject(shell);
        }
    }

    private sealed class Payload
    {
        public byte[] AppBytes;
        public byte[] DriverZipBytes;
        public byte[] LicenseBytes;
        public byte[] NoticeBytes;
        public byte[] ThirdPartyNoticesBytes;
        public byte[] ThirdPartyLicensesBytes;
        public string NoticeText;

        public static Payload Load()
        {
            Assembly assembly = Assembly.GetExecutingAssembly();
            Payload payload = new Payload();
            payload.AppBytes = ReadResource(assembly, AppResourceName);
            payload.DriverZipBytes = ReadResource(assembly, DriverResourceName);
            payload.LicenseBytes = ReadResource(assembly, LicenseResourceName);
            payload.NoticeBytes = ReadResource(assembly, NoticeResourceName);
            payload.ThirdPartyNoticesBytes = ReadResource(assembly, ThirdPartyNoticesResourceName);
            payload.ThirdPartyLicensesBytes = ReadResource(assembly, ThirdPartyLicensesResourceName);
            payload.NoticeText = Encoding.UTF8.GetString(payload.NoticeBytes);
            return payload;
        }

        private static byte[] ReadResource(Assembly assembly, string resourceName)
        {
            using (Stream stream = assembly.GetManifestResourceStream(resourceName))
            {
                if (stream == null)
                {
                    throw new InvalidDataException("O recurso esperado nao esta no instalador: " + resourceName);
                }

                using (MemoryStream output = new MemoryStream())
                {
                    stream.CopyTo(output);
                    return output.ToArray();
                }
            }
        }
    }

    private sealed class SetupDialog : Form
    {
        public SetupDialog(string notice)
        {
            Text = "Kynn Voice Setup";
            StartPosition = FormStartPosition.CenterScreen;
            FormBorderStyle = FormBorderStyle.FixedDialog;
            MaximizeBox = false;
            MinimizeBox = false;
            ClientSize = new Size(650, 380);

            Label title = new Label();
            title.Text = "Instalar Kynn Voice";
            title.Font = new Font(Font, FontStyle.Bold);
            title.Location = new Point(20, 16);
            title.Size = new Size(610, 28);
            Controls.Add(title);

            Label audioWarning = new Label();
            audioWarning.Text = "Atencao: o Windows 10/11 pode tornar o VB-CABLE a saida padrao depois da instalacao. Se acontecer, voce pode deixar de ouvir pelos alto-falantes. Leia as instrucoes abaixo antes de escolher App + VB-CABLE.";
            audioWarning.Font = new Font(Font, FontStyle.Bold);
            audioWarning.ForeColor = Color.DarkRed;
            audioWarning.AutoSize = false;
            audioWarning.Location = new Point(20, 48);
            audioWarning.Size = new Size(610, 54);
            Controls.Add(audioWarning);

            TextBox noticeBox = new TextBox();
            noticeBox.Multiline = true;
            noticeBox.ReadOnly = true;
            noticeBox.ScrollBars = ScrollBars.Vertical;
            noticeBox.Location = new Point(20, 105);
            noticeBox.Size = new Size(610, 167);
            noticeBox.Text = notice;
            Controls.Add(noticeBox);

            LinkLabel vendorLink = new LinkLabel();
            vendorLink.Text = "Site oficial da VB-Audio e informacoes de donationware";
            vendorLink.Location = new Point(20, 281);
            vendorLink.Size = new Size(610, 22);
            vendorLink.LinkClicked += delegate
            {
                ProcessStartInfo startInfo = new ProcessStartInfo("https://vb-cable.com/");
                startInfo.UseShellExecute = true;
                Process.Start(startInfo);
            };
            Controls.Add(vendorLink);

            Button driverButton = new Button();
            driverButton.Text = "App + VB-CABLE";
            driverButton.Location = new Point(20, 326);
            driverButton.Size = new Size(175, 36);
            driverButton.DialogResult = DialogResult.Yes;
            Controls.Add(driverButton);

            Button appOnlyButton = new Button();
            appOnlyButton.Text = "Somente o app";
            appOnlyButton.Location = new Point(218, 326);
            appOnlyButton.Size = new Size(155, 36);
            appOnlyButton.DialogResult = DialogResult.No;
            Controls.Add(appOnlyButton);

            Button cancelButton = new Button();
            cancelButton.Text = "Cancelar";
            cancelButton.Location = new Point(512, 326);
            cancelButton.Size = new Size(118, 36);
            cancelButton.DialogResult = DialogResult.Cancel;
            Controls.Add(cancelButton);

            AcceptButton = appOnlyButton;
            CancelButton = cancelButton;
        }
    }
}
