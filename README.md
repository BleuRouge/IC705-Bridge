# IC705 Bridge

Application de Tao GAUDONEIX pour connecter un Icom IC-705 en Wi-Fi,
échanger des trames CI-V dans un terminal et accéder à une API HTTP locale.

**[Télécharger la dernière version](https://github.com/BleuRouge/IC705-Bridge/releases/latest)**

| Système | Installeur |
| --- | --- |
| macOS — Intel et Apple Silicon | `.dmg` universel |
| Windows — x64 | `-setup.exe` ou `.msi` |
| Linux — x86_64 | `.AppImage`, `.deb` ou `.rpm` |

Les fichiers `.sig`, `.app.tar.gz` et `latest.json` servent aux mises à jour
intégrées. Les archives automatiques « Source code » de GitHub ne contiennent
que la documentation de ce dépôt ; elles ne servent pas à installer l'application.

## Démarrer

1. Connecter l'ordinateur au réseau de l'IC-705 et activer son accès RS-BA1.
2. Dans l'application, renseigner l'adresse de la radio et ses identifiants,
   puis cliquer sur **Se connecter**.
3. Utiliser **Terminal CI-V** pour envoyer vos trames et observer TX/ECHO/RX.
4. Pour Python, relever l'adresse de l'API locale puis suivre le
   **[guide étudiants](docs/ETUDIANTS.md)**.

Aucun paquet Python spécifique n'est nécessaire. Le guide explique uniquement
la communication HTTP ; la construction et le décodage des trames font partie du TP.

Les installeurs n'ont pas de certificat Apple ou Windows. Si le système bloque
l'ouverture, vérifier la provenance du fichier puis utiliser **Confidentialité
et sécurité → Ouvrir quand même** sur macOS ou **Informations complémentaires →
Exécuter quand même** dans SmartScreen. Sous Linux, rendre l'AppImage exécutable.

L'application propose les mises à jour au démarrage et vérifie leur signature.
La connexion à la radio reste utilisable sans accès à Internet.
L'onglet **Crédits** de la V1 présente l'auteur et la licence MIT.

Ce dépôt public contient la documentation étudiants et les fichiers distribués
dans les Releases. Le développement de l'application est conservé dans un dépôt privé.

[Signaler un problème](https://github.com/BleuRouge/IC705-Bridge/issues) · [Licence MIT](LICENSE)
