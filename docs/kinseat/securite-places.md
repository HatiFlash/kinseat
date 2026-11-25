# Sécurité des places : ce qu'un joueur peut faire à l'Hôte

Chaque place est un compte Windows local standard (non administrateur). Les stratégies ci-dessous sont écrites
uniquement dans la ruche du compte de place (`SeatLighteningService.ApplySeatSafetyPolicy` et `ApplyPowerPolicy`),
jamais dans HKLM ni dans la session de la console. Elles sont toujours appliquées, indépendamment des réglages
d'allégement, et sont idempotentes :
- au démarrage du service pour toutes les places gérées existantes (`SeatLighteningService.ApplySafetyPoliciesToAllSeats`),
  directement sur la ruche chargée `HKU\<SID>` si la session est active, sinon en montant `NTUSER.DAT` ;
- avant chaque ouverture ou reconnexion de session de place (`SessionLauncher.LaunchSessionAsync`) ainsi que lors
  de l'application de l'allègement (`SeatLighteningService.ApplyRegistry`).

Chaque application journalise une entrée `Info` (« Stratégies de sécurité appliquées à <compte> ») et un `Warning`
non bloquant en cas d'échec afin de ne jamais empêcher le démarrage du service.

## Tableau

Légende : STD = déjà bloqué par le compte standard ; MISSION = bloqué par ces stratégies utilisateur ;
DÉCISION = non bloquable par compte, choix nécessaire.

| Action dangereuse | Risque pour l'Hôte | Statut |
|---|---|---|
| Menus Démarrer / Alt+F4 sur le bureau / Ctrl+Alt+Suppr : Arrêter, Redémarrer, Veille, Veille prolongée | Coupe le PC pour tous | MISSION (`Explorer\NoClose=1`, existant). Se déconnecter reste possible. |
| `shutdown /s` / `/r` / `/h` en ligne de commande | Idem | LSA : groupe local dédié `Kinseat-ArretAutorise` peuplé des comptes non-places autorisés (locaux actifs, console, profils Microsoft/Entra/domaine), puis retrait des groupes larges de `SeShutdownPrivilege` ; sessions existantes à reconnecter. |
| `rundll32 powrprof.dll,SetSuspendState`, PowerShell `Stop-Computer` / `Restart-Computer`, API `ExitWindowsEx` d'un programme | Idem | Même protection LSA pour l'arrêt/redémarrage (et `SeRemoteShutdownPrivilege` pour l'arrêt distant) ; la veille et l'hibernation doivent être vérifiées sur l'Hôte Windows. |
| Mode Avion, coupure Wi-Fi / Bluetooth / réseau (zone de notification, réglages rapides) | Coupe le flux de toutes les places | MISSION partielle : `HideSCANetwork=1` retire l'icône réseau ; `NoControlPanel=1` bloque l'application Paramètres. Les réglages rapides de Windows 11 peuvent rester visibles selon la build : à vérifier au labo. Les actions sur les cartes réseau demandent l'élévation (STD). |
| Éjection de périphériques USB (« Retirer le périphérique en toute sécurité ») | Déconnecte un disque ou une manette d'une autre place | DÉCISION : aucune stratégie utilisateur ne retire l'icône sans bloquer le stockage USB (`RemovableStorageDevices`), ce qui gênerait les joueurs. Non implémenté. |
| Gestionnaire des tâches | Tuer Apollo, Kinseat ou un jeu d'une autre place (STD pour les processus d'autres comptes) | Autorisé volontairement : ne touche que les programmes du joueur (un compte standard ne peut pas terminer les processus d'un autre compte) et c'est le seul moyen de fermer un jeu figé. `DisableTaskMgr` n'est pas posé et est supprimé s'il l'avait été. |
| `regedit` | Modifier sa propre ruche (annulerait ces stratégies) | MISSION (`System\DisableRegistryTools=1`) + `regedit.exe` dans `DisallowRun`. HKLM : STD. |
| `cmd`, PowerShell, `mmc` | Lancer `shutdown`, scripts | cmd : volontairement NON bloqué (Steam et des lanceurs exécutent `cmd /c`). PowerShell, pwsh, ISE, mmc : `DisallowRun` (Explorateur seulement). Alternative : `Software\Policies\Microsoft\Windows\System\DisableCMD` (1 = cmd + .bat, 2 = cmd interactif seulement) après test avec les jeux de la bibliothèque. |
| Verrouiller / changer d'utilisateur / se déconnecter | Verrouiller sa propre session n'affecte pas l'Hôte ; « Changer d'utilisateur » peut basculer la console | Déconnexion volontairement laissée (`StartMenuLogOff` / `NoLogoff` non posés). Verrouillage / changement d'utilisateur : non traité, risque faible (DÉCISION si l'on veut `DisableLockWorkstation=1`). |
| Panneau de configuration / Paramètres (alimentation, réseau, Windows Update « Redémarrer maintenant ») | Changer le plan d'alimentation ou redémarrer | MISSION (`Explorer\NoControlPanel=1`). Le redémarrage d'une mise à jour demande des droits d'admin (STD). Conséquence : pas d'appairage Bluetooth depuis la place. |
| Installation de logiciels (MSI, pilotes, services) | Altération de l'Hôte | STD (élévation requise). Les installateurs par utilisateur restent possibles dans le profil de la place. |

## Valeurs posées (ruche du compte, `Software\Microsoft\Windows\CurrentVersion\Policies`)

| Clé | Valeur | Type | Donnée | Source |
|---|---|---|---|---|
| `Explorer` | `NoClose` | DWORD | 1 | Group Policy « Remove and prevent access to the Shut Down, Restart, Sleep, and Hibernate commands » |
| `Explorer` | `HideSCANetwork` | DWORD | 1 | ADMX Explorer « Remove the networking icon » |
| `Explorer` | `NoControlPanel` | DWORD | 1 | ADMX ControlPanel « Prohibit access to Control Panel and PC settings » |
| `Explorer` | `DisallowRun` | DWORD | 1 | ADMX Explorer « Don't run specified Windows applications » |
| `Explorer\DisallowRun` | `1`, `2`, … | chaîne | `shutdown.exe`, `powershell.exe`, `pwsh.exe`, `powershell_ise.exe`, `regedit.exe`, `mmc.exe` | idem (liste) |
| `System` | `DisableRegistryTools` | DWORD | 1 | ADMX « Prevent access to registry editing tools » |

Limite : `DisallowRun` ne s'applique qu'aux programmes lancés par l'Explorateur (Démarrer, Exécuter, raccourcis,
double-clic). Un processus lancé par un jeu, un script ou un autre programme n'est pas concerné.

## Droit d'arrêt machine et raccourcis publics

### Mécanisme et groupe local dédié (`Kinseat-ArretAutorise`)

Au démarrage et lors de la réconciliation périodique du service (`KinseatWorker`), `ShutdownRightPolicy`
protège `SeShutdownPrivilege` et `SeRemoteShutdownPrivilege` via un groupe local dédié nommé
`Kinseat-ArretAutorise` :
1. Le service crée ou maintient le groupe local `Kinseat-ArretAutorise` et y ajoute par SID :
   - tout compte local actif non-place (en excluant les comptes intégrés `Guest` `-501`, `DefaultAccount` `-503` et `WDAGUtilityAccount` `-504`) ;
   - le compte actuellement connecté sur la console ;
   - les comptes Microsoft, Entra ID (`S-1-12-1-*`) et de domaine (`S-1-5-21-*` hors préfixe machine locale) disposant d'un profil dans `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList`.
   Si l'ajout d'un seul SID au groupe échoue, le service journalise un `Warning` et ne retire aucun droit LSA.
2. Avant toute mutation LSA, l'état d'intention (droits, SID ajoutés, groupes larges retirés, nom et SID du groupe dédié) est persisté dans `C:\ProgramData\Kinseat\shutdown-rights\state.json` (répertoire restreint à `SYSTEM` et `Administrators`). L'état d'origine n'est jamais écrasé ; un indicateur `Applied` permet la reprise idempotente au démarrage suivant après un crash ou un échec partiel.
3. Le service accorde d'abord le droit au groupe `Kinseat-ArretAutorise` (ainsi qu'à `Administrators` `S-1-5-32-544` s'il était absent), puis retire UNIQUEMENT les groupes larges présents (`BUILTIN\Users` `S-1-5-32-545`, `Everyone` `S-1-1-0`, `Authenticated Users` `S-1-5-11`, `INTERACTIVE` `S-1-5-4`, `REMOTE INTERACTIVE` `S-1-5-14`). `Administrators`, `Backup Operators` (`S-1-5-32-551`), `SYSTEM`, les comptes de service et tout autre SID spécifique ne sont jamais retirés.
4. Si un compte de place géré est membre de `Administrators` ou `Backup Operators`, un `Warning` est journalisé et cette place n'est pas traitée comme protégée (sans être ajoutée à `Kinseat-ArretAutorise`).
5. La réconciliation périodique synchronise les membres du groupe `Kinseat-ArretAutorise` lorsqu'un nouveau compte famille, console ou profil Microsoft/Entra/domaine apparaît, sans modifier le journal d'origine.

### Réversibilité et désinstallation

- Lorsque plus aucune place gérée n'existe, `ShutdownRightPolicy.Restore` rétablit les groupes larges d'origine dans LSA, retire le groupe dédié des droits LSA, supprime le groupe local `Kinseat-ArretAutorise`, puis supprime `state.json`.
- Lors de la désinstallation (`Uninstall-Kinseat.ps1`), après arrêt du service et avant sa suppression, le script relit `state.json`, restaure les droits `USER_RIGHTS` via `secedit.exe`, vérifie la restauration par un second export, supprime le groupe local `Kinseat-ArretAutorise` puis supprime `state.json`. Les ACE `Deny Read` posées sur les raccourcis publics sont également retirées depuis l'inventaire `shortcuts.json`.
- En cas d'indisponibilité du service ou du désinstallateur, `state.json` conserve la liste exacte des attributions d'origine pour permettre un rétablissement manuel par un administrateur.

### Limites

- Les jetons de session déjà ouverts conservent leurs anciens privilèges jusqu'à déconnexion/reconnexion.
- Un compte Microsoft/Entra/domaine n'ayant pas encore de profil dans `ProfileList` et non connecté à la console au moment de la réconciliation ne rejoindra `Kinseat-ArretAutorise` qu'à la réconciliation suivante après création de son profil (un administrateur garde toujours `SeShutdownPrivilege`).
- Une place placée manuellement dans `Administrators` ou `Backup Operators`, ou disposant d'une attribution LSA directe, n'est pas bloquée (signalé en `Warning`).
- La protection complète (LSA, groupe dédié, `Uninstall-Kinseat.ps1` et comportement visuel des raccourcis) doit être vérifiée en direct sur l'Hôte Windows réel.

### Raccourcis publics

`SeatShortcutPolicy` place une ACL `Deny Read` uniquement sur `Kinseat Hôte.lnk` et `HidHide Configurator.lnk` présents sur le bureau public (`CommonDesktopDirectory`) et le menu Démarrer public (`CommonPrograms`), pour les SID des comptes de place vérifiés, et journalise chaque ACE dans `C:\ProgramData\Kinseat\shutdown-rights\shortcuts.json`. Les raccourcis de jeux ne sont pas touchés, les ACE obsolètes sont nettoyées lorsqu'une place est supprimée, et `Uninstall-Kinseat.ps1` retire les ACE enregistrées. Limite Windows : une ACL `Deny Read` sur un `.lnk` du bureau public empêche la lecture/l'exécution par la place mais peut laisser l'icône visible dans l'Explorateur selon la version de Windows.
