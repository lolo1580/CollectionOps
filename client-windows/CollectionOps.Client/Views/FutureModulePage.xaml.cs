using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace CollectionOps.Client.Views;

public sealed partial class FutureModulePage : Page
{
    public FutureModulePage() => InitializeComponent();

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        var documents = e.Parameter as string == "documents";
        ModuleTitle.Text = documents ? "Documents" : "Finances";
        ModuleDescription.Text = documents
            ? "Retrouvez les photographies, factures et archives liées à vos objets."
            : "Suivez la valeur et les mouvements financiers de votre collection.";
        DocumentsPreview.Visibility = documents ? Visibility.Visible : Visibility.Collapsed;
        FinancePreview.Visibility = documents ? Visibility.Collapsed : Visibility.Visible;
    }
}
