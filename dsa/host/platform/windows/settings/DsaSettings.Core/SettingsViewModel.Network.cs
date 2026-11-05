namespace Dsa.Settings.Core;

/// <summary>The Network page: how Android reaches the network now, and the advanced networking
/// switch.</summary>
public sealed partial class SettingsViewModel
{
    private NetworkInfo? _network;

    /// <summary>The network as last read: from <c>dsa network</c> after a refresh, else with the
    /// settings.</summary>
    public NetworkInfo? Network => _network ?? _settings?.Network;

    /// <summary>Whether Android joins this computer's network as a device of its own. Off by
    /// default.</summary>
    public bool AdvancedNetworking => _settings?.AdvancedNetworking ?? false;

    /// <summary>The mode, as the page names it.</summary>
    public string NetworkModeText => (Network?.Mode ?? NetworkMode.Unknown) switch
    {
        NetworkMode.Nat => _text.Get("Network_ModeNat"),
        NetworkMode.Advanced => _text.Get("Network_ModeAdvanced"),
        NetworkMode.Off => _text.Get("Network_ModeOff"),
        _ => _text.Get("Network_ModeUnknown"),
    };

    /// <summary>Whether Android has a network whose addresses the page can show.</summary>
    public bool HasNetworkDetails => Network is { Mode: NetworkMode.Nat or NetworkMode.Advanced };

    private string JoinOrNone(IReadOnlyList<string>? items) =>
        items is { Count: > 0 } ? string.Join(", ", items) : _text.Get("Network_None");

    public string NetworkAddresses => JoinOrNone(Network?.Addresses);

    public string NetworkDns => JoinOrNone(Network?.DnsServers);

    public string NetworkGateway => Network?.Gateway is { Length: > 0 } gateway ? gateway : _text.Get("Network_None");

    public string NetworkProblem => Network?.Problem ?? "";

    public bool HasNetworkProblem => !string.IsNullOrEmpty(Network?.Problem);

    /// <summary>Whether the switch differs from the network Android has now, which changes only
    /// when Android starts again. A problem explains a difference of its own.</summary>
    public bool NetworkRestartNeeded => Network is { Mode: NetworkMode.Nat or NetworkMode.Advanced } network
        && network.Problem is null
        && AdvancedNetworking != (network.Mode == NetworkMode.Advanced);

    /// <summary>Reads Android's network again. A service that cannot say leaves what the
    /// settings reported.</summary>
    public async Task RefreshNetworkAsync()
    {
        try
        {
            _network = await _service.NetworkAsync().ConfigureAwait(true);
        }
        catch (DsaException)
        {
            _network = null;
        }
        ChangedAll();
    }

    public async Task SetAdvancedNetworkingAsync(bool on)
    {
        if (on == AdvancedNetworking && Loaded)
        {
            return;
        }
        // The settings answer carries the network as it stands, which the page shows next.
        _network = null;
        await ChangeAsync(() => _service.SetAdvancedNetworkingAsync(on)).ConfigureAwait(true);
    }
}
