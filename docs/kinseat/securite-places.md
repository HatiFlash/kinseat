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
| `shutdown /s` / `/r` / `/h` en ligne de commande | Idem | DÉCISION : un compte standard a `SeShutdownPrivilege` (groupe Utilisateurs). `NoClose` ne l'empêche pas. `DisallowRun` bloque `shutdown.exe` seulement lancé depuis l'Explorateur. |
| `rundll32 powrprof.dll,SetSuspendState`, PowerShell `Stop-Computer` / `Restart-Computer`, API `ExitWindowsEx` d'un programme | Idem | DÉCISION (même cause). PowerShell est bloqué depuis l'Explorateur (`DisallowRun`), pas depuis un processus lancé par un jeu. |
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

## Décision nécessaire : le droit d'arrêt

`SeShutdownPrivilege` (« Arrêter le système ») est accordé par défaut au groupe Utilisateurs. Aucune stratégie
utilisateur ne le retire à un seul compte. Le retirer pour le groupe Utilisateurs est une stratégie locale de la
machine entière (`secpol.msc` → Attribuer des droits d'utilisateur → Arrêter le système), ce qui touche aussi
Hatim et la console de l'Hôte. Solution proposée : ne laisser ce droit qu'aux Administrateurs et au compte de la
console, ou refuser le droit au groupe local dédié aux places de Kinseat (`SeDenyShutdownPrivilege` n'existe pas :
il faut donc retirer Utilisateurs et accorder à un groupe « Console »). Non implémenté ici ; à valider avant,
car un retrait mal fait peut empêcher les mises à jour planifiées et l'arrêt par le service.
