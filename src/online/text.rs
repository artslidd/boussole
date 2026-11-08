//! French texts of the online mode that do not depend on the screen layout.

use boussole_protocol::ErrorCode;

pub const SESSION_EXPIRED: &str =
    "La partie n’est plus disponible : vous avez été retiré de la salle.";
pub const ROOM_CLOSED: &str = "La salle a été fermée.";
pub const SERVER_UNREACHABLE: &str =
    "Serveur injoignable : vérifiez l’adresse, l’hôte et votre connexion.";
pub const HOST_CLOSED: &str = "L’hôte a fermé la partie.";
pub const OWN_SERVER_STOPPED: &str = "Le serveur de la partie s’est arrêté.";
pub const SERVER_FULL: &str =
    "Salle pleine : le serveur n’accepte plus de connexions pour le moment.";
pub const TOO_MANY_CONNECTIONS: &str = "Trop de connexions depuis cette adresse : fermez les autres fenêtres du jeu ou réessayez dans un instant.";
pub const CANNOT_CREATE_ROOM: &str =
    "Le serveur ne peut pas créer de salle pour le moment (limite atteinte) : réessayez plus tard.";
pub const CONNECTION_LOST: &str = "Connexion perdue : impossible de rejoindre le serveur.";

/// What the player reads for a server error. Starts from the protocol's own French text and
/// rewords the few that need a client-side hint.
pub fn error_fr(code: ErrorCode) -> String {
    match code {
        ErrorCode::BadVersion => "Version incompatible — mettez le jeu à jour.".to_owned(),
        ErrorCode::DatasetMismatch => {
            "Liste des villes incompatible — mettez le jeu à jour (ou celui de l’hôte).".to_owned()
        }
        ErrorCode::RoomNotFound => {
            "Salle introuvable : vérifiez le code (5 caractères) et l’adresse.".to_owned()
        }
        ErrorCode::RateLimited => {
            "Trop d’actions en peu de temps : patientez un instant.".to_owned()
        }
        other => other.message_fr().to_owned(),
    }
}
