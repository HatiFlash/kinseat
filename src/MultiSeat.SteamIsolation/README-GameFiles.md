MultiSeat.GameFiles — jalon A (Windows, MSVC C++17)

Commande : MultiSeat.SteamLauncher.exe --game "C:\ProgramData\MultiSeat\games\joueur\jeu.json" -- "D:\Jeux\jeu.exe" [arguments]

Exemple de manifeste UTF-8 (écrit par le service, lecture seule pour le joueur) :

{
  "gameId": "jeu", "account": "joueur", "accountSid": "S-1-5-21-...",
  "profileDir": "C:\\Users\\joueur",
  "redirects": [{"from": "C:\\Users\\Public\\Documents", "to": "C:\\Users\\joueur\\AppData\\Local\\Kinseat\\Sauvegardes", "mode": "redirect"}],
  "copyFromHostOnce": false,
  "logFile": "C:\\Users\\joueur\\AppData\\Local\\Kinseat\\gamefiles.log"
}

Le lanceur vérifie le SID, démarre l'exécutable suspendu dans un Job, injecte la DLL d'architecture correspondante, puis le reprend. SID absent ou différent : aucun hook, démarrage direct dans le Job ; diagnostic dans le journal du lanceur. En mode `--game`, son code de sortie est celui du processus principal du jeu (ou du lanceur 32 bits délégué) ; le mode Steam conserve son comportement propre. La DLL revérifie le SID et lit MULTISEAT_GAME_MANIFEST transmis par le lanceur. Elle réécrit les chemins des ouvertures, créations, requêtes d'attributs, suppressions, renommages et liens NT dans les préfixes configurés. Le journal du jeu note les chemins et erreurs, jamais le contenu ; rotation vers .1 à 1 Mio (le dossier du journal doit déjà exister).

Mode overlay : pour chaque préfixe `from` (jeu) / `to` (sauvegardes), les lectures voient la copie `to` si elle existe, sinon l'original `from`. Une première écriture sur un fichier ou dossier uniquement dans l'original effectue un copy-up vers `to` (les fichiers sont copiés avant l'opération d'écriture). Les suppressions de l'original sont représentées par des chemins relatifs inscrits dans `to\\.kinseat-deleted` ; ce marqueur masque l'entrée de l'original, sans effacer le fichier hôte. L'énumération d'un répertoire fusionne les noms de `to` puis `from`, sans doublons (insensible à la casse) ; les entrées de l'overlay prévalent et les marqueurs masquent celles de l'original. Le fichier `.kinseat-deleted` lui-même n'est pas exposé.

Limites : l'union concerne les appels d'énumération NT pris en charge et n'est pas une implémentation complète d'un système de fichiers union. Les opérations sur handles ouverts avant injection peuvent ne pas être isolées ; les renommages/liens n'offrent pas une sémantique transactionnelle complète. Les marqueurs utilisent une ligne UTF-8 par chemin (les noms contenant un retour à la ligne ne sont pas représentables). La copie d'un fichier peut échouer, notamment si la destination existe déjà ou si les droits/espace disque manquent ; aucune garantie d'atomicité ou de synchronisation entre plusieurs processus. Pas de protection contre les jonctions/liens symboliques : le manifeste doit rester non modifiable par le joueur et les cibles être préparées par le service.


Compilation : .\build.ps1, produit les DLL x64/x86. Les tests de chemins et du manifeste sont lancés pour chaque architecture ; CTest : ctest --test-dir build-x64 -C Release --output-on-failure. Les deux DLL et les deux lanceurs doivent être côte à côte. Aucune installation ni intégration au service dans ce jalon.

Limites : l'overlay n'est pas un système de fichiers union complet : pas de synchronisation transactionnelle entre processus ni de garantie face aux jonctions/liens symboliques ; le manifeste doit rester non modifiable par le joueur et les cibles préparées par le service. Les préfixes sont des chemins de lecteur absolus ; UNC, chemins de périphérique, chemins relatifs au lecteur et flux alternatifs ne sont pas redirigés. Les alias 8.3 ne sont pas développés. Les chemins relatifs à un handle racine dont le nom n'est pas résoluble restent inchangés. Les enfants de même architecture créés via CreateProcessInternalW sont injectés avant leur reprise, y compris si l'appelant demande CREATE_SUSPENDED ; un échec d'injection les arrête. Un bloc d'environnement explicite sans MULTISEAT_GAME_MANIFEST identique au manifeste actif est refusé (pas de fusion implicite). Les enfants d'autre architecture restent non isolés (journalisation et reprise selon la demande de l'appelant) ; CreateProcessWithLogon/Token et les processus démarrés hors descendance ne sont pas couverts. Ne pas annoncer une isolation complète avant qualification VM 261 et jalon B.
