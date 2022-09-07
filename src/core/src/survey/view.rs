use crate::math::projection::HEALPix;
use crate::{coosys, healpix::cell::HEALPixCell, math};
use std::collections::HashMap;

use crate::math::angle::Angle;
use crate::Projection;
use cgmath::Vector2;
use al_core::{log, inforec};
pub fn project_vertices<P: Projection>(cell: &HEALPixCell, camera: &CameraViewPort) -> [Vector2<f64>; 4] {
    let project_vertex = |(lon, lat): (f64, f64)| -> Vector2<f64> {
        let vertex = crate::math::lonlat::radec_to_xyzw(Angle(lon), Angle(lat));
        P::view_to_screen_space(&vertex, camera).unwrap()
    };
    
    let vertices = cell.vertices();

    [
        project_vertex(vertices[0]),
        project_vertex(vertices[1]),
        project_vertex(vertices[2]),
        project_vertex(vertices[3])
    ]
}

// Compute a depth from a number of pixels on screen
pub fn depth_from_pixels_on_screen<P: Projection>(camera: &CameraViewPort, num_pixels: i32) -> u8 {
    let width = camera.get_screen_size().x;
    let aperture = camera.get_aperture().0 as f32;

    let angle_per_pixel = aperture / width;

    let two_power_two_times_depth_pixel =
        std::f32::consts::PI / (3.0 * angle_per_pixel * angle_per_pixel);
    let depth_pixel = (two_power_two_times_depth_pixel.log2() / 2.0).round() as u32;

    //let survey_max_depth = conf.get_max_depth();
    // The depth of the texture
    // A texture of 512x512 pixels will have a depth of 9
    let depth_offset_texture = math::utils::log_2_unchecked(num_pixels);
    // The depth of the texture corresponds to the depth of a pixel
    // minus the offset depth of the texture
    if depth_offset_texture > depth_pixel {
        0_u8
    } else {
        (depth_pixel - depth_offset_texture) as u8
    }

    /*let mut depth = 0;
    let mut d1 = std::f64::MAX;
    let mut d2 = std::f64::MAX;

    while d1 > 512.0*512.0 && d2 > 512.0*512.0 {

        let (lon, lat) = crate::math::lonlat::xyzw_to_radec(camera.get_center());
        let lonlat = math::lonlat::LonLatT(lon, lat);

        let (ipix, _, _) = crate::healpix::utils::hash_with_dxdy(depth, &lonlat);
        let vertices = project_vertices::<P>(&HEALPixCell(depth, ipix), camera);

        d1 = crate::math::vector::dist2(&vertices[0], &vertices[2]);
        d2 = crate::math::vector::dist2(&vertices[1], &vertices[3]);

        depth += 1;
    }
    al_core::info!(depth);
    if depth > 0 {
        depth - 1
    } else {
        0
    }*/
}
use crate::healpix;
use al_api::coo_system::CooSystem;
use cgmath::Vector4;
use crate::survey::HEALPixCoverage;
pub fn get_tile_cells_in_camera(
    depth_tile: u8,
    camera: &CameraViewPort,
    hips_frame: CooSystem,
) -> (HEALPixCoverage, Vec<HEALPixCell>) {
    let coverage = if depth_tile <= 1 {
        HEALPixCoverage::allsky(depth_tile)
    } else {
        if let Some(vertices) = camera.get_vertices() {
            // The vertices coming from the camera are in a specific coo sys
            // but cdshealpix accepts them to be given in ICRSJ2000 coo sys
            let view_system = camera.get_system();
            let icrsj2000_fov_vertices_pos = vertices
                .iter()
                .map(|v| coosys::apply_coo_system(view_system, &hips_frame, v))
                .collect::<Vec<_>>();

            let vs_inside_pos = camera.get_center();
            let icrsj2000_inside_pos =
                coosys::apply_coo_system(view_system, &hips_frame, &vs_inside_pos);

            // Prefer to query from_polygon with depth >= 2
            healpix::coverage::HEALPixCoverage::new(
                depth_tile,
                &icrsj2000_fov_vertices_pos[..],
                &icrsj2000_inside_pos.truncate(),
            )
        } else {
            HEALPixCoverage::allsky(depth_tile)
        }
    };

    let cells = coverage.flatten_to_fixed_depth_cells()
        .map(|idx| {
            HEALPixCell(depth_tile, idx)
        })
        .collect();
    
    //al_core::log(&format!("cells: {:?}", cells));
    
    (coverage, cells)
}

// Contains the cells being in the FOV for a specific
pub struct HEALPixCellsInView {
    // The set of cells being in the current view for a
    // specific image survey
    pub depth: u8,
    prev_depth: u8,
    look_for_parents: bool,

    // flags associating true to cells that
    // are new in the fov
    cells: HashMap<HEALPixCell, bool>,
    // A flag telling whether there has been
    // new cells added from the last frame
    is_new_cells_added: bool,

    coverage: HEALPixCoverage,
}

use crate::camera::{CameraViewPort, UserAction};
impl HEALPixCellsInView {
    pub fn new() -> Self {
        let cells = HashMap::new();
        let coverage = HEALPixCoverage::allsky(0);

        Self {
            cells,
            prev_depth: 0,
            depth: 0,
            look_for_parents: false,
            is_new_cells_added: false,
            coverage,
        }
    }

    pub fn reset_frame(&mut self) {
        self.is_new_cells_added = false;
        self.prev_depth = self.get_depth();
    }

    // This method is called whenever the user does an action
    // that moves the camera.
    // Everytime the user moves or zoom, the views must be updated
    // The new cells obtained are used for sending new requests
    pub fn refresh(&mut self, new_depth: u8, hips_frame: CooSystem, camera: &CameraViewPort) {
        self.depth = new_depth;

        // Get the cells of that depth in the current field of view
        let (coverage, tile_cells) = get_tile_cells_in_camera(self.depth, camera, hips_frame);
        self.coverage = coverage;

        // Update cells in the fov
        self.update_cells_in_fov(&tile_cells, camera);
    }

    fn update_cells_in_fov(&mut self, cells_in_fov: &[HEALPixCell], camera: &CameraViewPort) {
        let new_cells = cells_in_fov
            .iter()
            .map(|cell| {
                let new = !self.cells.contains_key(cell);
                self.is_new_cells_added |= new;

                (*cell, new)
            })
            .collect::<HashMap<_, _>>();
        self.cells = new_cells;

        if camera.get_last_user_action() == UserAction::Unzooming {
            self.look_for_parents = self.has_depth_decreased();
        } else {
            self.look_for_parents = false;
        }
    }

    // Accessors
    #[inline]
    pub fn get_cells(&self) -> impl Iterator<Item = &HEALPixCell> {
        self.cells.keys()
    }

    #[inline]
    pub fn num_of_cells(&self) -> usize {
        self.cells.len()
    }

    #[inline]
    pub fn get_depth(&self) -> u8 {
        self.depth
    }

    #[inline]
    pub fn is_new(&self, cell: &HEALPixCell) -> bool {
        if let Some(&is_cell_new) = self.cells.get(cell) {
            is_cell_new
        } else {
            false
        }
    }

    #[inline]
    pub fn get_coverage(&self) -> &HEALPixCoverage {
        &self.coverage
    }

    #[inline]
    pub fn is_there_new_cells_added(&self) -> bool {
        //self.new_cells.is_there_new_cells_added()
        self.is_new_cells_added
    }

    /*#[inline]
    pub fn has_depth_decreased_while_unzooming(&self, camera: &CameraViewPort) -> bool {
        debug_assert!(camera.get_last_user_action() == UserAction::Unzooming);
        self.look_for_parents
    }*/

    #[inline]
    fn has_depth_decreased(&self) -> bool {
        let depth = self.get_depth();
        depth < self.prev_depth
    }
}
