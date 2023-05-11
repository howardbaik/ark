//
// r-data-viewer.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::event::CommEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use harp::object::RObject;
use harp::r_lock;
use harp::utils::r_is_null;
use harp::vector::CharacterVector;
use harp::vector::Vector;
use libR_sys::NILSXP;
use libR_sys::R_NamesSymbol;
use libR_sys::Rf_getAttrib;
use libR_sys::STRSXP;
use libR_sys::VECTOR_ELT;
use libR_sys::XLENGTH;
use serde::Deserialize;
use serde::Serialize;
use stdext::spawn;
use uuid::Uuid;

use crate::lsp::globals::comm_manager_tx;

pub struct RDataViewer {
    pub id: String,
    pub title: String,
    pub data: RObject,
    pub comm: CommSocket,
}

#[derive(Deserialize, Serialize)]
pub struct DataColumn {
    pub name: String,

    #[serde(rename = "type")]
    pub column_type: String,

    pub data: Vec<String>
}

#[derive(Deserialize, Serialize)]
pub struct DataSet {
    pub id: String,
    pub title: String,
    pub columns: Vec<DataColumn>,

    #[serde(rename = "rowCount")]
    pub row_count: usize
}

impl DataSet {
    pub fn from_object(id: String, title: String, object: RObject) -> Result<Self, harp::error::Error> {

        let columns = r_lock! {
            let mut columns = vec![];

            let names = Rf_getAttrib(*object, R_NamesSymbol);
            if r_is_null(names) {
                return Err(harp::error::Error::UnexpectedType(NILSXP, vec![STRSXP]))
            }
            let names = CharacterVector::new_unchecked(names);

            let n_columns = XLENGTH(*object);
            for i in 0..n_columns {
                let data = harp::vector::format(VECTOR_ELT(*object, i));

                columns.push(DataColumn{
                    name: names.get_unchecked(i).unwrap(),
                    column_type: String::from("String"),
                    data
                });
            }

            Ok(columns)
        }?;

        Ok(Self {
            id,
            title,
            columns,
            row_count: 0
        })

    }
}

impl RDataViewer {

    pub fn start(title: String, data: RObject) {
        spawn!("ark-data-viewer", move || {
            let id = Uuid::new_v4().to_string();
            let comm = CommSocket::new(
                CommInitiator::BackEnd,
                id.clone(),
                String::from("positron.dataViewer"),
            );
            let viewer = Self {
                id,
                title,
                data,
                comm
            };
            viewer.execution_thread()
        });
    }

    pub fn execution_thread(self) -> Result<(), anyhow::Error> {
        let data_set = DataSet::from_object(self.id.clone(), self.title, self.data)?;
        let json = serde_json::to_value(data_set)?;

        let comm_manager_tx = comm_manager_tx();
        let event = CommEvent::Opened(self.comm.clone(), json);
        comm_manager_tx.send(event)?;

        // TODO: some sort of select!() loop to listen for events from the comm

        Ok(())
    }

}
