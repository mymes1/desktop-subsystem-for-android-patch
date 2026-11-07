using System.Windows;
using System.Windows.Automation;
using System.Windows.Automation.Peers;
using System.Windows.Automation.Provider;
using System.Windows.Media;
using Wpf.Ui.Controls;

namespace Dsa.Settings;

/// <summary>An item of the navigation pane that UI Automation clients can find and use.
/// The library's item gives automation no name and no way to select it, so a screen reader
/// announced an unnamed element and could not open a page with it. This item is named by its
/// label, and selecting or invoking it opens its page, as a click does. It looks the same:
/// the library's style and the pane's template still apply.</summary>
public class PaneItem : NavigationViewItem
{
    protected override AutomationPeer OnCreateAutomationPeer() => new PaneItemAutomationPeer(this);

    protected override void OnPropertyChanged(DependencyPropertyChangedEventArgs e)
    {
        base.OnPropertyChanged(e);
        if (e.Property == IsActiveProperty
            && UIElementAutomationPeer.FromElement(this) is PaneItemAutomationPeer peer)
        {
            peer.RaisePropertyChangedEvent(SelectionItemPatternIdentifiers.IsSelectedProperty,
                e.OldValue is true, e.NewValue is true);
            peer.RaiseAutomationEvent(e.NewValue is true
                ? AutomationEvents.SelectionItemPatternOnElementSelected
                : AutomationEvents.SelectionItemPatternOnElementRemovedFromSelection);
        }
    }

    /// <summary>Opens this item's page in the navigation view that holds it.</summary>
    internal void Open()
    {
        if (!IsEnabled)
        {
            throw new ElementNotEnabledException();
        }
        if (TargetPageType is not { } page)
        {
            return;
        }
        DependencyObject? node = this;
        while (node is not null and not NavigationView)
        {
            node = VisualTreeHelper.GetParent(node) ?? LogicalTreeHelper.GetParent(node);
        }
        (node as NavigationView)?.Navigate(page);
    }
}

/// <summary>A list item named by the pane item's label, selected while its page shows.</summary>
public sealed class PaneItemAutomationPeer : FrameworkElementAutomationPeer, ISelectionItemProvider, IInvokeProvider
{
    public PaneItemAutomationPeer(PaneItem owner) : base(owner)
    {
    }

    private PaneItem Item => (PaneItem)Owner;

    protected override string GetClassNameCore() => nameof(PaneItem);

    protected override AutomationControlType GetAutomationControlTypeCore() => AutomationControlType.ListItem;

    protected override string GetNameCore()
    {
        var name = base.GetNameCore();
        return string.IsNullOrEmpty(name) ? Item.Content?.ToString() ?? "" : name;
    }

    protected override bool IsKeyboardFocusableCore() => Item.Focusable;

    public override object? GetPattern(PatternInterface patternInterface) => patternInterface switch
    {
        PatternInterface.SelectionItem or PatternInterface.Invoke => this,
        _ => base.GetPattern(patternInterface),
    };

    public bool IsSelected => Item.IsActive;

    public IRawElementProviderSimple? SelectionContainer => null;

    public void Select() => Item.Open();

    public void AddToSelection()
    {
        // One page shows at a time, so adding to the selection is selecting.
        if (!Item.IsActive)
        {
            Item.Open();
        }
    }

    public void RemoveFromSelection()
    {
        // A page stays open until another opens: there is no empty selection to go back to.
        if (Item.IsActive)
        {
            throw new InvalidOperationException("one page is always open; select another item instead");
        }
    }

    public void Invoke() => Item.Open();
}
