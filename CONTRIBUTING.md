# Contribuer à CollectionOps

## Principes

- Une modification doit rester rattachable à un milestone et à un besoin validé.
- Aucun secret, mot de passe ou jeton ne doit être ajouté au dépôt.
- Le client ne doit jamais accéder directement à MariaDB.
- Les changements de contrat API doivent mettre à jour OpenAPI, les tests et la documentation.
- Toute décision structurante doit être consignée dans un ADR sous `docs/architecture/`.

## Vérifications avant contribution

### Backend

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

### Client Windows

Depuis un terminal développeur Visual Studio sous Windows :

```powershell
dotnet restore client-windows/CollectionOps.Client/CollectionOps.Client.csproj
dotnet build client-windows/CollectionOps.Client/CollectionOps.Client.csproj -c Release -p:Platform=x64
```

## Changelog

Chaque contribution ayant un effet visible, architectural, opérationnel ou de sécurité doit ajouter une entrée dans la section `Non publié` de `CHANGELOG.md`. Les corrections purement typographiques peuvent en être dispensées.

