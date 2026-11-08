using System.Windows.Automation.Peers;
using System.Windows.Controls;

namespace Dsa.Settings;

/// <summary>A list of rows on a settings page, such as the widgets, the apps or the memory
/// figures, or in the APK Installer's window, such as the permissions. The APK Installer builds
/// this same file, so both apps' lists read alike. A plain ItemsControl puts each row behind a generic data item, and UI Automation's
/// control view, which screen readers and the installer test read, showed nothing inside those
/// items: the Widgets page read as headings with no widget under them although every row was
/// drawn. This list exposes its rows' own text, buttons and switches directly as its
/// descendants, as the rest of the page does.</summary>
public class RowList : ItemsControl
{
    protected override AutomationPeer OnCreateAutomationPeer() => new RowListAutomationPeer(this);
}

/// <summary>A list whose children are what its rows draw. The element peer walks the visual
/// tree, and a row's container has no peer of its own, so the walk reaches each row's
/// controls.</summary>
public sealed class RowListAutomationPeer : FrameworkElementAutomationPeer
{
    public RowListAutomationPeer(RowList owner) : base(owner)
    {
    }

    protected override string GetClassNameCore() => nameof(RowList);

    protected override AutomationControlType GetAutomationControlTypeCore() => AutomationControlType.Group;
}
