# Fake game window for smoke tests: a plain coloured window titled like the game.
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$form = New-Object System.Windows.Forms.Form
$form.Text = if ($env:ENI_FAKE_TITLE) { $env:ENI_FAKE_TITLE } else { "Path of Exile 2" }
$form.Width = 1000
$form.Height = 640
$form.StartPosition = "Manual"
$form.Left = if ($env:ENI_FAKE_X) { [int]$env:ENI_FAKE_X } else { 80 }
$form.Top = if ($env:ENI_FAKE_Y) { [int]$env:ENI_FAKE_Y } else { 80 }
$form.BackColor = [System.Drawing.Color]::FromArgb(28, 52, 78)
$form.ShowInTaskbar = $false
$form.TopMost = $true
$label = New-Object System.Windows.Forms.Label
$label.Text = "fake game window"
$label.ForeColor = [System.Drawing.Color]::FromArgb(220, 200, 150)
$label.Font = New-Object System.Drawing.Font("Segoe UI", 18)
$label.AutoSize = $true
$label.Left = 40
$label.Top = 40
$form.Controls.Add($label)
# A test window must never be minimized: GetClientRect of a minimized window is 0x0.
$timer = New-Object System.Windows.Forms.Timer
$timer.Interval = 500
$timer.Add_Tick({
  if ($form.WindowState -eq [System.Windows.Forms.FormWindowState]::Minimized) {
    $form.WindowState = [System.Windows.Forms.FormWindowState]::Normal
  }
  if (-not $form.Visible) { $form.Show() }
})
$timer.Start()
$form.Add_Shown({ $form.Activate() })
[System.Windows.Forms.Application]::Run($form)
