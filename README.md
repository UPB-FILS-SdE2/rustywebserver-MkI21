[![Review Assignment Due Date](https://classroom.github.com/assets/deadline-readme-button-22041afd0340ce965d47ae6ef1cefeee28c7c493a6346c4f15d667ab976d596c.svg)](https://classroom.github.com/a/TXciPqtn)
# Rustwebserver

Detail the homework implementation.

pour lancer le serveur:
```bash
rustywebserver PORT <Folder>
```

La fonction async fn main est l'entrée principale de l'application et utilise tokio pour gérer les tâches asynchrones. Elle collecte les arguments de la ligne de commande pour obtenir le port et le répertoire racine, configure un écouteur TCP pour accepter les connexions entrantes, et lance des tâches asynchrones pour chaque connexion pour traiter les requêtes.

La fonction async fn handle_request traite les requêtes HTTP en lisant les données de la connexion, analysant la méthode HTTP et le chemin de la requête, et en répondant en fonction de la méthode (GET ou POST). Elle vérifie si le fichier demandé existe, est interdit, ou s'il s'agit d'un script à exécuter. Elle envoie une réponse appropriée au client selon la situation (fichier, répertoire, script, etc.).

La fonction async fn send_response envoie une réponse HTTP au client en formatant les en-têtes et le corps de la réponse en fonction des paramètres fournis, puis écrit la réponse dans le flux de connexion.

La fonction async fn execute_script exécute des scripts à partir du serveur en configurant les variables d'environnement nécessaires, en exécutant le script avec ces variables, et en envoyant le résultat du script comme réponse HTTP.

La fonction fn is_forbidden_file vérifie si un fichier est interdit d'accès en s'assurant que le fichier n'est pas en dehors du répertoire racine et en vérifiant contre une liste de motifs interdits.

La fonction async fn generate_directory_listing génère une liste de répertoires sous forme de HTML en parcourant les fichiers et répertoires dans le répertoire demandé et en les formatant en une liste HTML.

La fonction async fn read_file lit le contenu d'un fichier et retourne ce contenu sous forme de vecteur d'octets.

La fonction fn get_mime_type détermine le type MIME d'un fichier en fonction de son extension pour spécifier le type de contenu dans la réponse HTTP.

La fonction async fn log_connection enregistre les informations sur la connexion, y compris la méthode HTTP, l'adresse IP du client, le chemin demandé, et le code de statut de la réponse.

```bash
cargo build
export PATH=/workspaces/rustywebserver-MkI21/target/debug:$PATH
rustywebserver 8000 ./public
```
