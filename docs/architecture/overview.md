# Architecture initiale

```text
Client Windows (WinUI 3)
        |
        | HTTPS / API v1
        v
Backend Linux (Rust / Axum)
   |                   |
   | SQLx              | service documentaire
   v                   v
MariaDB externe    stockage local + S3
```

## Règles structurantes

1. Aucun client ne communique directement avec MariaDB.
2. Le backend applique systématiquement authentification, autorisation et audit.
3. Les contrats API sont versionnés et décrits en OpenAPI.
4. Le cache local du client est considéré comme une réplique partielle chiffrée, jamais comme la source centrale.
5. La synchronisation, les conflits et les révocations feront l’objet d’un protocole documenté avant leur implémentation.
6. Les documents sont adressés par métadonnées ; leur stockage physique reste abstrait du domaine métier.

## État du socle

Le backend expose uniquement un contrôle de santé et son document OpenAPI. Le client affiche une coquille de navigation sans authentification, persistance ou communication réseau. Aucune base de données ni infrastructure n’est créée par ce socle.

