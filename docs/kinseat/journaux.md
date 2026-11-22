# Journaux et rapport de diagnostic Kinseat

- Service : `%ProgramData%\Kinseat\logs\service-*.log` (journal avec rotation). Niveau Normal ou Détaillé dans Paramètres → Assistance.
- Agent de chaque place : `%LOCALAPPDATA%\Kinseat\agent.log` **dans le profil de la place**, avec `agent.log.1` si présent.
- Application hôte : `%LOCALAPPDATA%\Kinseat\logs\kinseat-app.log` (deux fichiers de 5 Mio maximum : courant et `kinseat-app.log.1`). Sous Linux : `$XDG_STATE_HOME/kinseat/kinseat-app.log` ou `~/.local/state/kinseat/kinseat-app.log`.
- Installation : `%ProgramData%\Kinseat\logs\setup-*.log`.

Les lignes de l'application sont horodatées en UTC. Les actions sont consignées sans message saisi, mot de passe ou contenu des réponses IPC ; les erreurs IPC sont consignées sans leurs détails. Le niveau Détaillé ajoute les actualisations de l'état du service. Une panique enregistre son emplacement, sans son message potentiellement sensible.

## Créer et envoyer le rapport

Ouvrir Paramètres → Assistance, choisir le niveau souhaité, puis « Créer un rapport de diagnostic ». Patienter jusqu'à « Rapport créé » ; le chemin du `.zip` apparaît. « Ouvrir le dossier » sélectionne le fichier dans l'Explorateur Windows. Le rapport est enregistré dans `%ProgramData%\Kinseat\diagnostics\` et reste local : aucun envoi automatique. Joindre **ce fichier `.zip`** au signalement, après en avoir vérifié le contenu.

L'archive contient `LISEZ-MOI.txt`, `etat.json` (machine, santé, places, avis, paramètres), jusqu'à dix journaux récents du service, les journaux disponibles des agents et d'Apollo pour les places connues, le journal courant de l'application hôte et, sur Windows, des événements pertinents des journaux Application et Système. Le contenu texte est expurgé des secrets connus avant archivage ; les mots de passe stockés, codes PIN et clés ne sont pas ajoutés. Certains journaux peuvent être absents s'ils sont indisponibles ou trop volumineux. La limite des données d'entrée de l'archive est de 45 Mio. Les journaux `setup-*.log` ne sont **pas** inclus automatiquement : joindre séparément celui de l'installation si le problème concerne le setup, après relecture.
