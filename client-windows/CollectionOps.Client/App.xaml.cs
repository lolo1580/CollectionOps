using CollectionOps.Client.Services;
using Microsoft.UI.Xaml;

namespace CollectionOps.Client;

public partial class App : Application
{
    public static SessionApi Sessions { get; } = new();
    public static MainWindow? MainWindow { get; private set; }
    private Window? _window;

    public App()
    {
        InitializeComponent();
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        _window = new MainWindow();
        MainWindow = (MainWindow)_window;
        _window.Activate();
    }
}

