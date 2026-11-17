# IC705 Bridge

Application permettant de connecter un Icom IC-705 en Wi-Fi,
échanger des trames CI-V dans un terminal et accéder à une API HTTP locale.

L’utilisation des exécutables est réservée aux étudiants, enseignants et personnels du département GEII de l’IUT de Bordeaux dans le cadre de ses activités pédagogiques. Voir la [licence propriétaire](LICENSE).

**[Télécharger la dernière version](https://github.com/BleuRouge/IC705-Bridge/releases/latest)**

| Système | Installeur |
| --- | --- |
| macOS — Intel et Apple Silicon | `.dmg` universel |
| Windows — x64 | `-setup.exe` ou `.msi` |
| Linux — x86_64 | `.AppImage`, `.deb` ou `.rpm` |

Les fichiers `.sig`, `.app.tar.gz` et `latest.json` servent aux mises à jour
intégrées. Les archives automatiques « Source code » de GitHub ne contiennent
que la documentation de ce dépôt ; elles ne servent pas à installer l'application.

Pour l’AppImage Linux, l’archive `linux-third-party-sources.tar` de la release fournit les sources et notices des bibliothèques embarquées.

## Démarrer

1. Ouvrir IC705 Bridge avec Internet. L’application vérifie la version publiée
   sur GitHub et installe automatiquement les mises à jour signées.
2. Une fois la vérification terminée, rejoindre le Wi-Fi de l’IC-705 et activer
   son accès RS-BA1.
3. Renseigner l’adresse de la radio et ses identifiants, puis cliquer sur
   **Se connecter**.
4. Utiliser **Terminal CI-V** pour envoyer vos trames et observer TX/ECHO/RX.
   Pour Python, suivre le **[guide étudiants](docs/ETUDIANTS.md)**.

Vous pouvez relancer la même version hors ligne pendant les 24 heures qui
suivent sa dernière vérification en ligne. Si l’application détecte une
nouvelle version, elle attend son installation avant de donner accès à la
radio et à l’API locale.

Les installeurs n'ont pas de certificat Apple ou Windows. Si le système bloque
l'ouverture, vérifier la provenance du fichier puis utiliser **Confidentialité
et sécurité → Ouvrir quand même** sur macOS ou **Informations complémentaires →
Exécuter quand même** dans SmartScreen. Sous Linux, rendre l'AppImage exécutable.

[Signaler un problème](https://github.com/BleuRouge/IC705-Bridge/issues) · [Licence propriétaire](LICENSE) · [Notices des composants tiers](THIRD_PARTY_NOTICES.md)
